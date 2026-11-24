//! Fixed-capacity (at most 4) stack-allocated block list for Moonlight FEC frames.
//! Moonlight's 2-bit block-count wire field limits every frame to 1..=4 FEC blocks,
//! so keeping them inline avoids per-frame heap allocations on the hot path.
use std::ops::{Deref, DerefMut};

/// Up to 4 FEC blocks stored inline on the stack.
#[derive(Clone, Debug)]
pub struct InlineBlocks<T> {
    items: [T; 4],
    len: u8,
}

impl<T: Copy> Copy for InlineBlocks<T> {}

impl<T: Default> Default for InlineBlocks<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Default> InlineBlocks<T> {
    pub fn new() -> Self {
        Self {
            items: std::array::from_fn(|_| T::default()),
            len: 0,
        }
    }

    pub fn one(item: T) -> Self {
        let mut out = Self::new();
        out.push(item);
        out
    }

    /// `len` default-initialized elements (at most 4).
    pub fn zeros(len: usize) -> Self {
        assert!(len <= 4, "Moonlight supports at most 4 FEC blocks");
        let mut out = Self::new();
        out.len = len as u8;
        out
    }

    /// Removes and returns the elements from `at..`, leaving `0..at` in `self`.
    pub fn split_off(&mut self, at: usize) -> Self {
        let len = self.len as usize;
        assert!(at <= len, "split_off index out of bounds");
        let mut tail = Self::new();
        for i in at..len {
            tail.push(std::mem::take(&mut self.items[i]));
        }
        self.len = at as u8;
        tail
    }
}

impl<T> InlineBlocks<T> {
    pub fn push(&mut self, item: T) {
        let len = self.len as usize;
        assert!(len < 4, "Moonlight supports at most 4 FEC blocks");
        self.items[len] = item;
        self.len = (len + 1) as u8;
    }

    pub fn as_slice(&self) -> &[T] {
        &self.items[..self.len as usize]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items[..self.len as usize]
    }
}

impl<T> Deref for InlineBlocks<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> DerefMut for InlineBlocks<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: PartialEq> PartialEq for InlineBlocks<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for InlineBlocks<T> {}

impl<T: PartialEq> PartialEq<Vec<T>> for InlineBlocks<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: PartialEq> PartialEq<InlineBlocks<T>> for Vec<T> {
    fn eq(&self, other: &InlineBlocks<T>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: PartialEq, const N: usize> PartialEq<[T; N]> for InlineBlocks<T> {
    fn eq(&self, other: &[T; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: PartialEq> PartialEq<&[T]> for InlineBlocks<T> {
    fn eq(&self, other: &&[T]) -> bool {
        self.as_slice() == *other
    }
}

impl<T: Default> FromIterator<T> for InlineBlocks<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut out = Self::new();
        for item in iter {
            out.push(item);
        }
        out
    }
}

impl<T> IntoIterator for InlineBlocks<T> {
    type Item = T;
    type IntoIter = std::iter::Take<std::array::IntoIter<T, 4>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len as usize)
    }
}

impl<'a, T> IntoIterator for &'a InlineBlocks<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, T> IntoIterator for &'a mut InlineBlocks<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}
