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
use std::borrow::Cow;

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

/// Own `bytes` without copying when it is already an owned buffer (such as a
/// pooled header rewrite); copy borrowed bytes once with `headroom` for later
/// metadata splicing.
fn into_owned(bytes: Cow<'_, [u8]>, headroom: usize) -> Vec<u8> {
    match bytes {
        Cow::Owned(owned) => owned,
        Cow::Borrowed(slice) => frame_pool::copy_with_headroom(slice, headroom),
    }
}

/// Append `bytes` into `joined`, moving an owned buffer directly when `joined`
/// is still empty and it already meets `hint`, or returning an owned buffer to
/// the pool after copying its bytes into `joined`.
fn append_cow(joined: &mut Vec<u8>, bytes: Cow<'_, [u8]>, hint: usize) {
    match bytes {
        Cow::Owned(owned) if joined.is_empty() && owned.capacity() >= hint => {
            *joined = owned;
        }
        Cow::Owned(owned) => {
            append(joined, &owned, hint);
            give(owned);
        }
        Cow::Borrowed(slice) => append(joined, slice, hint),
    }
}

/// Accepted byte inputs for [`Sequencer::push`] in unit tests, preserving
/// ergonomic slice/array/`&Vec<u8>` calls alongside zero-copy `Cow::Owned`.
#[cfg(test)]
pub trait UnitBytes<'a> {
    fn into_cow(self) -> Cow<'a, [u8]>;
}
#[cfg(test)]
impl<'a> UnitBytes<'a> for Cow<'a, [u8]> {
    fn into_cow(self) -> Cow<'a, [u8]> {
        self
    }
}
#[cfg(test)]
impl<'a> UnitBytes<'a> for Vec<u8> {
    fn into_cow(self) -> Cow<'a, [u8]> {
        Cow::Owned(self)
    }
}
#[cfg(test)]
impl<'a> UnitBytes<'a> for &'a [u8] {
    fn into_cow(self) -> Cow<'a, [u8]> {
        Cow::Borrowed(self)
    }
}
#[cfg(test)]
impl<'a> UnitBytes<'a> for &'a Vec<u8> {
    fn into_cow(self) -> Cow<'a, [u8]> {
        Cow::Borrowed(self.as_slice())
    }
}
#[cfg(test)]
impl<'a, const N: usize> UnitBytes<'a> for &'a [u8; N] {
    fn into_cow(self) -> Cow<'a, [u8]> {
        Cow::Borrowed(self.as_slice())
    }
}

impl Sequencer {
    /// Take the next buffer AMF returned. `kind` is its `OutputBufferType`
    /// (1 slice, 2 last slice; anything else is a whole frame). `first` runs
    /// on a frame's first unit only. Owned buffers are moved without copying.
    #[cfg(test)]
    pub fn push<'a>(
        &mut self,
        pts: i64,
        kind: Option<i64>,
        bytes: impl UnitBytes<'a>,
        first: impl FnOnce() -> Meta,
    ) -> Pushed {
        self.push_with_headroom(pts, kind, bytes.into_cow(), 0, first)
    }
    /// Take the next buffer AMF returned (`kind`: 1 slice, 2 last slice, other
    /// whole frame), moving owned buffers directly and reserving `headroom`
    /// extra bytes whenever borrowed input is copied so a subsequent dynamic
    /// HDR metadata splice does not reallocate.
    pub fn push_with_headroom(
        &mut self,
        pts: i64,
        kind: Option<i64>,
        bytes: Cow<'_, [u8]>,
        headroom: usize,
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
                step: Step::Whole(into_owned(bytes, headroom)),
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
            Step::Unit(unit, into_owned(bytes, headroom))
        } else if !last {
            let hint = self.hint[usize::from(meta.idr)].max(bytes.len() + headroom);
            append_cow(&mut open.joined, bytes, hint);
            Step::Hold
        } else if index == 0 {
            Step::Whole(into_owned(bytes, headroom))
        } else {
            let mut joined = std::mem::take(&mut open.joined);
            let needed = joined.len() + bytes.len() + headroom;
            append_cow(&mut joined, bytes, needed);
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
        let pushed = sequencer.push(8, Some(2), b"x".as_slice(), || JOINED);
        assert_eq!(pushed.step, Step::Whole(b"x".to_vec()));
        assert!(pushed.meta.idr && !pushed.meta.streamed);
        assert_eq!(pushed.abandoned, None);
    }

    #[test]
    fn the_first_unit_meta_is_kept_for_the_whole_frame() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), [1u8].as_slice(), || JOINED);
        let pushed = sequencer.push(1, Some(2), [2u8].as_slice(), || panic!("decided twice"));
        assert_eq!(pushed.meta, JOINED);
        assert_eq!(pushed.step, Step::Whole(vec![1, 2]));
    }

    #[test]
    fn a_frame_returned_whole_passes_through_in_either_mode() {
        let mut sequencer = Sequencer::default();
        for kind in [None, Some(0)] {
            let pushed = sequencer.push(3, kind, b"frame".as_slice(), || STREAMED);
            assert_eq!(pushed.step, Step::Whole(b"frame".to_vec()));
            assert!(!pushed.meta.streamed);
        }
    }

    #[test]
    fn a_frame_without_its_last_unit_is_reported_when_the_next_arrives() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"a".as_slice(), || STREAMED);
        let pushed = sequencer.push(2, Some(1), b"b".as_slice(), || STREAMED);
        assert_eq!(pushed.abandoned, Some(1));
        assert_eq!(
            pushed.step,
            Step::Unit(Unit::Slice { index: 0 }, b"b".to_vec())
        );
        // A joined frame's leftovers never reach the next one.
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"stale".as_slice(), || JOINED);
        let pushed = sequencer.push(2, Some(2), b"new".as_slice(), || JOINED);
        assert_eq!(pushed.abandoned, Some(1));
        assert_eq!(pushed.step, Step::Whole(b"new".to_vec()));
    }

    #[test]
    fn forgetting_a_frame_drops_its_units_only() {
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), b"a".as_slice(), || JOINED);
        sequencer.forget(2);
        assert!(sequencer.open.is_some());
        sequencer.forget(1);
        let pushed = sequencer.push(1, Some(2), b"b".as_slice(), || JOINED);
        assert_eq!(pushed.step, Step::Whole(b"b".to_vec()));
        assert_eq!(pushed.abandoned, None);
    }

    #[test]
    fn a_large_joined_keyframe_is_gathered_in_one_pooled_buffer_once_its_size_is_known() {
        let _alone = frame_pool::alone();
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
        sequencer.push(3, Some(1), b"aa".as_slice(), || JOINED_INTER);
        assert!(sequencer.open.as_ref().unwrap().joined.capacity() < POOLED);
        sequencer.forget(3);
        frame_pool::give(second);
    }

    #[test]
    fn an_unfinished_joined_frame_gives_its_buffer_back_however_it_ends() {
        let _alone = frame_pool::alone();
        let slice = vec![9; POOLED];
        let joined = |sequencer: &Sequencer| sequencer.open.as_ref().unwrap().joined.capacity();
        given();
        // Abandoned when the next frame's unit arrives.
        let mut sequencer = Sequencer::default();
        sequencer.push(1, Some(1), &slice, || JOINED);
        let capacity = joined(&sequencer);
        assert_eq!(
            sequencer
                .push(2, Some(1), b"x".as_slice(), || STREAMED)
                .abandoned,
            Some(1)
        );
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
        let Step::Whole(whole) = sequencer.push(5, None, b"frame".as_slice(), || JOINED).step
        else {
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

    #[test]
    fn owned_rewritten_units_move_without_copying_and_return_to_pool_when_joined() {
        let _alone = frame_pool::alone();
        let mut sequencer = Sequencer::default();
        given();
        // Streamed unit moves the owned buffer directly without copying.
        let owned = vec![0x11; 64];
        let ptr = owned.as_ptr();
        let Step::Unit(Unit::Slice { index: 0 }, moved) = sequencer
            .push(10, Some(1), Cow::Owned(owned), || STREAMED)
            .step
        else {
            panic!("expected streamed slice");
        };
        assert_eq!(moved.as_ptr(), ptr);
        assert!(given().is_empty());
        // Single-unit whole frame moves the owned buffer directly.
        let owned = vec![0x22; 64];
        let ptr = owned.as_ptr();
        let Step::Whole(moved) = sequencer
            .push(11, Some(0), Cow::Owned(owned), || STREAMED)
            .step
        else {
            panic!("expected whole frame");
        };
        assert_eq!(moved.as_ptr(), ptr);
        // Multi-unit joined frame returns a second owned buffer to the pool after copying.
        let first = vec![0x33; POOLED];
        let first_ptr = first.as_ptr();
        let second = vec![0x44; POOLED];
        let second_cap = second.capacity();
        assert_eq!(
            sequencer
                .push(12, Some(1), Cow::Owned(first), || JOINED)
                .step,
            Step::Hold
        );
        assert_eq!(sequencer.open.as_ref().unwrap().joined.as_ptr(), first_ptr);
        let Step::Whole(joined) = sequencer
            .push(12, Some(2), Cow::Owned(second), || JOINED)
            .step
        else {
            panic!("expected joined frame");
        };
        assert_eq!(joined.len(), POOLED * 2);
        let returned = given();
        assert!(returned.contains(&second_cap));
        frame_pool::give(joined);
        // Borrowed slice with headroom leaves spare capacity for metadata splicing.
        let Step::Unit(Unit::Slice { index: 0 }, with_room) = sequencer
            .push_with_headroom(13, Some(1), Cow::Borrowed(b"slice"), 256, || STREAMED)
            .step
        else {
            panic!("expected streamed slice");
        };
        assert_eq!(with_room, b"slice");
        assert!(with_room.capacity() >= 5 + 256);
    }
}
