//! The output units of a frame in AMF's slice output mode.
//!
//! QueryOutput returns one buffer per slice, typed 1 (slice) or 2 (last slice
//! of the frame). A frame that may be sent block by block leaves unit by unit.
//! Any other (a keyframe, one after an invalidation, one with no byte cap, or
//! any frame once the host turned slice delivery off) is joined and returned
//! whole at its last unit, as frame output mode would have returned it.
//!
//! Every unit and frame is copied into a buffer from [`crate::frame_pool`],
//! as frame output mode copies its frames, so a large keyframe reuses the
//! same memory each time instead of mapping it again.

use crate::{encoder::Unit, frame_pool};

/// What the first unit of a frame decides for all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Units leave one by one; otherwise they are joined.
    pub streamed: bool,
    /// The frame is a keyframe.
    pub idr: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// A unit of a frame that is joined: nothing to hand on yet.
    Hold,
    /// One unit of a streamed frame.
    Unit(Unit, Vec<u8>),
    /// A whole frame: a joined one, or one AMF returned in frame mode.
    Whole(Vec<u8>),
}

#[derive(Debug)]
pub struct Pushed {
    pub step: Step,
    /// Set by the frame's first unit and kept to its last.
    pub meta: Meta,
    /// The frame still open when this unit came belonged to another PTS: it
    /// never got its last unit.
    pub abandoned: Option<i64>,
}

struct Open {
    pts: i64,
    next: u32,
    meta: Meta,
    joined: Vec<u8>,
}
/// A frame that never gets its last unit, or ends with the encoder, gives
/// the buffer its units were joined in back to the pool.
impl Drop for Open {
    fn drop(&mut self) {
        give(std::mem::take(&mut self.joined));
    }
}

#[derive(Default)]
pub struct Sequencer {
    open: Option<Open>,
    /// How large a buffer the next joined frame starts in, by whether it is
    /// a keyframe: the last such frame's size and a quarter more.
    hint: [usize; 2],
}

fn give(buffer: Vec<u8>) {
    #[cfg(test)]
    tests::GIVEN.with(|given| given.borrow_mut().push(buffer.capacity()));
    frame_pool::give(buffer);
}

/// Appends `bytes` to a joined frame. A buffer that is too small is replaced
/// by a pooled one of at least `hint` (or twice its size) and given back, so
/// joining never grows a buffer outside the pool.
fn append(joined: &mut Vec<u8>, bytes: &[u8], hint: usize) {
    let needed = joined.len() + bytes.len();
    if needed > joined.capacity() {
        let mut grown = frame_pool::take(needed.max(hint).max(joined.capacity() * 2));
        grown.extend_from_slice(joined);
        give(std::mem::replace(joined, grown));
    }
    joined.extend_from_slice(bytes);
}

impl Sequencer {
    /// Take the next buffer AMF returned. `kind` is its `OutputBufferType`
    /// (1 slice, 2 last slice; anything else is a whole frame). `first` runs
    /// on a frame's first unit only.
    pub fn push(
        &mut self,
        pts: i64,
        kind: Option<i64>,
        bytes: &[u8],
        first: impl FnOnce() -> Meta,
    ) -> Pushed {
        let abandoned = match &self.open {
            Some(open) if open.pts != pts => self.open.take().map(|open| open.pts),
            _ => None,
        };
        if !matches!(kind, Some(1 | 2)) {
            let meta = first();
            self.open = None;
            return Pushed {
                step: Step::Whole(frame_pool::copy(bytes)),
                meta: Meta {
                    streamed: false,
                    ..meta
                },
                abandoned,
            };
        }
        let last = kind == Some(2);
        let open = self.open.get_or_insert_with(|| Open {
            pts,
            next: 0,
            meta: first(),
            joined: Vec::new(),
        });
        let index = open.next;
        open.next += 1;
        let meta = open.meta;
        let step = if meta.streamed {
            let unit = if last {
                Unit::Last { index }
            } else {
                Unit::Slice { index }
            };
            Step::Unit(unit, frame_pool::copy(bytes))
        } else if !last {
            append(&mut open.joined, bytes, self.hint[usize::from(meta.idr)]);
            Step::Hold
        } else if index == 0 {
            Step::Whole(frame_pool::copy(bytes))
        } else {
            let mut joined = std::mem::take(&mut open.joined);
            append(&mut joined, bytes, 0);
            self.hint[usize::from(meta.idr)] = joined.len() + joined.len() / 4;
            Step::Whole(joined)
        };
        if last {
            self.open = None;
        }
        Pushed {
            step,
            meta,
            abandoned,
        }
    }
    /// A frame AMF never finished: forget its units.
    pub fn forget(&mut self, pts: i64) {
        if self.open.as_ref().is_some_and(|open| open.pts == pts) {
            self.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_pool::POOLED;
    use std::cell::RefCell;

    thread_local! {
        /// The capacities of the buffers this thread gave back to the pool.
        pub(super) static GIVEN: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }
    /// The non-empty buffers given back since the last call.
    fn given() -> Vec<usize> {
        GIVEN
            .with(|given| std::mem::take(&mut *given.borrow_mut()))
            .into_iter()
            .filter(|&capacity| capacity > 0)
            .collect()
    }

    const STREAMED: Meta = Meta {
        streamed: true,
        idr: false,
    };
    const JOINED: Meta = Meta {
        streamed: false,
        idr: true,
    };
    const JOINED_INTER: Meta = Meta {
        streamed: false,
        idr: false,
    };

    fn run(sequencer: &mut Sequencer, pts: i64, meta: Meta, units: &[(i64, &[u8])]) -> Vec<Step> {
        let mut calls = 0;
        let steps = units
            .iter()
            .map(|&(kind, bytes)| {
                sequencer
                    .push(pts, Some(kind), bytes, || {
                        calls += 1;
                        meta
                    })
                    .step
            })
            .collect();
        assert_eq!(calls, 1, "the first unit alone decides the frame");
        steps
    }

    #[test]
    fn a_streamed_frame_leaves_unit_by_unit() {
        let mut sequencer = Sequencer::default();
        let steps = run(
            &mut sequencer,
            7,
            STREAMED,
            &[(1, b"aa"), (1, b"bbb"), (2, b"c")],
        );
        assert_eq!(
            steps,
            [
                Step::Unit(Unit::Slice { index: 0 }, b"aa".to_vec()),
                Step::Unit(Unit::Slice { index: 1 }, b"bbb".to_vec()),
                Step::Unit(Unit::Last { index: 2 }, b"c".to_vec()),
            ]
        );
        assert!(sequencer.open.is_none());
    }

    #[test]
    fn a_joined_frame_returns_whole_at_its_last_unit() {
        let mut sequencer = Sequencer::default();
        let steps = run(
            &mut sequencer,
            7,
            JOINED,
            &[(1, b"aa"), (1, b"bbb"), (2, b"c")],
        );
        assert_eq!(
            steps,
            [Step::Hold, Step::Hold, Step::Whole(b"aabbbc".to_vec())]
        );
        let pushed = sequencer.push(8, Some(2), b"x", || JOINED);
        assert_eq!(pushed.step, Step::Whole(b"x".to_vec()));
        assert!(pushed.meta.idr && !pushed.meta.streamed);
        assert_eq!(pushed.abandoned, None);
    }

    #[test]
    fn the_first_unit_meta_is_kept_for_the_whole_frame() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), &[1], || JOINED);
        let pushed = sequencer.push(1, Some(2), &[2], || panic!("decided twice"));
        assert_eq!(pushed.meta, JOINED);
        assert_eq!(pushed.step, Step::Whole(vec![1, 2]));
    }

    #[test]
    fn a_frame_returned_whole_passes_through_in_either_mode() {
        let mut sequencer = Sequencer::default();
        for kind in [None, Some(0)] {
            let pushed = sequencer.push(3, kind, b"frame", || STREAMED);
            assert_eq!(pushed.step, Step::Whole(b"frame".to_vec()));
            assert!(!pushed.meta.streamed);
        }
    }

    #[test]
    fn a_frame_without_its_last_unit_is_reported_when_the_next_arrives() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"a", || STREAMED);
        let pushed = sequencer.push(2, Some(1), b"b", || STREAMED);
        assert_eq!(pushed.abandoned, Some(1));
        assert_eq!(
            pushed.step,
            Step::Unit(Unit::Slice { index: 0 }, b"b".to_vec())
        );
        // A joined frame's leftovers never reach the next one.
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"stale", || JOINED);
        let pushed = sequencer.push(2, Some(2), b"new", || JOINED);
        assert_eq!(pushed.abandoned, Some(1));
        assert_eq!(pushed.step, Step::Whole(b"new".to_vec()));
    }

    #[test]
    fn forgetting_a_frame_drops_its_units_only() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"a", || JOINED);
        sequencer.forget(2);
        assert!(sequencer.open.is_some());
        sequencer.forget(1);
        let pushed = sequencer.push(1, Some(2), b"b", || JOINED);
        assert_eq!(pushed.step, Step::Whole(b"b".to_vec()));
        assert_eq!(pushed.abandoned, None);
    }

    #[test]
    fn a_large_joined_keyframe_is_gathered_in_one_pooled_buffer_once_its_size_is_known() {
        let slice = vec![5; POOLED / 2 + 1];
        let mut sequencer = Sequencer::default();
        given();
        // The first keyframe grows through pooled buffers, giving each back.
        for _ in 0..3 {
            sequencer.push(1, Some(1), &slice, || JOINED);
        }
        let Step::Whole(first) = sequencer.push(1, Some(2), &slice, || JOINED).step else {
            panic!("a joined frame is whole at its last unit");
        };
        assert_eq!(first.len(), slice.len() * 4);
        assert!(first.capacity() >= POOLED);
        let grown = given();
        assert!(!grown.is_empty() && grown.iter().all(|&capacity| capacity < first.capacity()));
        frame_pool::give(first);
        // The next one starts in a buffer that holds it: no growth, nothing given.
        sequencer.push(2, Some(1), &slice, || JOINED);
        let open = sequencer.open.as_ref().unwrap();
        let (address, capacity) = (open.joined.as_ptr(), open.joined.capacity());
        assert!(capacity >= slice.len() * 4);
        for _ in 0..2 {
            sequencer.push(2, Some(1), &slice, || JOINED);
        }
        let Step::Whole(second) = sequencer.push(2, Some(2), &slice, || JOINED).step else {
            panic!("a joined frame is whole at its last unit");
        };
        assert_eq!(second.as_ptr(), address);
        assert_eq!(second, vec![5; slice.len() * 4]);
        assert_eq!(given(), Vec::<usize>::new());
        // An inter frame keeps its own size and does not take a keyframe's buffer.
        sequencer.push(3, Some(1), b"aa", || JOINED_INTER);
        assert!(sequencer.open.as_ref().unwrap().joined.capacity() < POOLED);
        sequencer.forget(3);
        frame_pool::give(second);
    }

    #[test]
    fn an_unfinished_joined_frame_gives_its_buffer_back_however_it_ends() {
        let slice = vec![9; POOLED];
        let joined = |sequencer: &Sequencer| sequencer.open.as_ref().unwrap().joined.capacity();
        given();
        // Abandoned when the next frame's unit arrives.
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), &slice, || JOINED);
        let capacity = joined(&sequencer);
        assert_eq!(sequencer.push(2, Some(1), b"x", || STREAMED).abandoned, Some(1));
        assert_eq!(given(), [capacity]);
        // Forgotten after an output error.
        sequencer.forget(2);
        sequencer.push(3, Some(1), &slice, || JOINED);
        let capacity = joined(&sequencer);
        sequencer.forget(3);
        assert_eq!(given(), [capacity]);
        // Replaced by a frame AMF returned whole.
        sequencer.push(4, Some(1), &slice, || JOINED);
        let capacity = joined(&sequencer);
        let Step::Whole(whole) = sequencer.push(5, None, b"frame", || JOINED).step else {
            panic!("a frame returned whole passes through");
        };
        assert_eq!(whole, b"frame");
        assert_eq!(given(), [capacity]);
        // Dropped with the encoder, as a session ends mid-frame.
        sequencer.push(6, Some(1), &slice, || JOINED);
        let capacity = joined(&sequencer);
        drop(sequencer);
        assert_eq!(given(), [capacity]);
    }
}
