//! The reference pictures of an encode session and the loss recovery built on them. The session
//! keeps its last few reconstructed pictures. Each P picture refers to the newest one it still
//! holds, which is the picture before it until a client reports loss: then every held picture
//! from the first lost one on is let go, so the next picture refers to the last one before the
//! loss and the decoder is repaired without a keyframe. When none is held that old, the answer
//! is a keyframe, as it is for the Windows encoders ([`butterpollo_media_policy::nvenc`]).
use super::ffi::VASurfaceID;
use std::collections::VecDeque;

/// The most pictures a session holds as references. The decoder's picture buffer, which the
/// level bounds, has to take them and the picture being decoded.
pub const MAX_REFERENCES: usize = 4;

/// One reconstructed picture kept as a reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    pub surface: VASurfaceID,
    /// The picture's number on the wire, which a client names when it lost it.
    pub wire: u64,
    /// Pictures between the last IDR picture and this one: the picture order count and, in
    /// H.264, the frame number.
    pub order: u32,
}

pub struct References {
    capacity: u32,
    /// Oldest first.
    held: VecDeque<Held>,
    /// The wire number the next picture gets, counting from 1.
    next: u64,
    /// The wire numbers of the last picture and of the last IDR picture; 0 before the first.
    last: u64,
    last_idr: u64,
    /// The order of the picture after the last one, to tell which held pictures a decoder's
    /// window still has.
    next_order: u32,
    /// The range the last invalidation covered, so a repeated report does nothing.
    covered: Option<(u64, u64)>,
    /// The next picture is the first made after an invalidation.
    recovering: bool,
}

impl References {
    /// A holder of at most `capacity` pictures.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity as u32,
            held: VecDeque::with_capacity(capacity + 1),
            next: 1,
            last: 0,
            last_idr: 0,
            next_order: 0,
            covered: None,
            recovering: false,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    /// The wire number the first picture of a rebuilt encoder gets.
    pub fn set_next(&mut self, wire: u64) {
        self.next = wire.max(1);
    }

    pub fn next(&self) -> u64 {
        self.next
    }

    /// Lets go of the pictures a decoder's sliding window has dropped by the time the picture
    /// numbered `order` is decoded, and returns what is held, oldest first.
    pub fn held_for(&mut self, order: u32) -> Vec<Held> {
        let capacity = self.capacity;
        self.held
            .retain(|h| order.saturating_sub(h.order) <= capacity);
        self.held.iter().copied().collect()
    }

    fn slide(&mut self, order: u32) {
        let capacity = self.capacity;
        self.held
            .retain(|h| order.saturating_sub(h.order) <= capacity);
    }

    /// A surface of `all` that holds no reference, to reconstruct the next picture into.
    pub fn free(&self, all: &[VASurfaceID]) -> VASurfaceID {
        *all.iter()
            .find(|id| self.held.iter().all(|h| h.surface != **id))
            .expect("the session has a surface more than it holds")
    }

    /// Takes a finished picture in. Returns whether it is the first after an invalidation.
    pub fn commit(&mut self, surface: VASurfaceID, order: u32, idr: bool) -> bool {
        let wire = self.next;
        self.next += 1;
        self.last = wire;
        self.next_order = order + 1;
        if idr {
            self.held.clear();
            self.last_idr = wire;
            self.covered = None;
            self.recovering = false;
        }
        self.held.push_back(Held {
            surface,
            wire,
            order,
        });
        while self.held.len() > self.capacity as usize {
            self.held.pop_front();
        }
        std::mem::take(&mut self.recovering)
    }

    /// Frames `first` to `last` (wire numbers) did not reach the client, and neither can any
    /// later picture that depends on them be trusted. True when later pictures now repair that
    /// without a keyframe; false when only a keyframe does.
    pub fn invalidate(&mut self, first: u64, last: u64) -> bool {
        if first == 0 || last < first || last > self.last {
            return false;
        }
        // Delayed feedback for an earlier GOP cannot harm this one. A lost IDR picture has no
        // anchor of its own.
        if last < self.last_idr {
            return true;
        }
        if first <= self.last_idr {
            return false;
        }
        if self
            .covered
            .is_some_and(|(start, end)| first >= start && last <= end)
        {
            return true;
        }
        self.slide(self.next_order);
        if self.held.iter().all(|h| h.wire >= first) {
            return false;
        }
        self.held.retain(|h| h.wire < first);
        self.covered = Some((first, self.last));
        self.recovering = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `count` pictures, the first an IDR picture; surface ids are 10 + slot.
    fn run(refs: &mut References, count: u32) {
        let all: Vec<VASurfaceID> = (10..10 + refs.capacity() as u32 + 1).collect();
        let start = refs.held.back().map_or(0, |h| h.order + 1);
        for n in 0..count {
            let order = start + n;
            let _ = refs.held_for(order);
            let surface = refs.free(&all);
            refs.commit(surface, order, order == 0);
        }
    }

    #[test]
    fn holds_the_last_pictures() {
        let mut refs = References::new(4);
        run(&mut refs, 7);
        let wires: Vec<u64> = refs.held.iter().map(|h| h.wire).collect();
        assert_eq!(wires, [4, 5, 6, 7]);
        assert_eq!(refs.next(), 8);
    }

    #[test]
    fn a_report_drops_the_lost_pictures_and_repairs_from_the_one_before() {
        let mut refs = References::new(4);
        run(&mut refs, 7);
        assert!(refs.invalidate(5, 6));
        let wires: Vec<u64> = refs.held.iter().map(|h| h.wire).collect();
        assert_eq!(wires, [4]);
        // The same report again changes nothing, and the next picture is the recovery one.
        assert!(refs.invalidate(5, 6));
        assert!(refs.commit(10, 7, false));
        assert!(!refs.commit(11, 8, false));
    }

    #[test]
    fn a_loss_older_than_the_pictures_held_needs_a_keyframe() {
        let mut refs = References::new(4);
        run(&mut refs, 12);
        // Pictures 9 to 12 are held; 8 is gone.
        assert!(!refs.invalidate(9, 9));
        assert!(!refs.invalidate(3, 4));
        // Beyond what exists, or malformed.
        assert!(!refs.invalidate(13, 13));
        assert!(!refs.invalidate(0, 3));
        assert!(!refs.invalidate(6, 5));
    }

    #[test]
    fn the_idr_picture_cannot_be_repaired_and_older_gops_do_not_matter() {
        let mut refs = References::new(4);
        run(&mut refs, 3);
        assert!(!refs.invalidate(1, 1));
        refs.commit(20, 0, true);
        refs.commit(21, 1, false);
        // Picture 4 is the IDR picture: a loss from it on needs a keyframe, before it nothing.
        assert!(!refs.invalidate(4, 5));
        assert!(refs.invalidate(2, 3));
    }

    #[test]
    fn a_report_counts_only_what_the_next_picture_can_still_refer_to() {
        let mut refs = References::new(4);
        run(&mut refs, 3);
        assert!(refs.invalidate(3, 3));
        // Pictures 4 and 5 are made from picture 2; picture 1, order 0, is held still but the
        // decoder's window has slid past it by the time the picture of order 5 is decoded.
        refs.commit(30, 3, false);
        refs.commit(31, 4, false);
        assert!(!refs.invalidate(2, 5));
        assert!(refs.invalidate(4, 5));
    }

    #[test]
    fn the_decoder_window_bounds_what_is_referred_to() {
        let mut refs = References::new(4);
        run(&mut refs, 3);
        assert!(refs.invalidate(3, 3));
        // Two pictures are held (1, 2). Far later the decoder has slid past them.
        assert_eq!(refs.held_for(2).len(), 2);
        assert_eq!(refs.held_for(7).len(), 0);
    }
}
