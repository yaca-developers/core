//! An `Iterator` extension that yields only the last `n` items of a sequence.
//!
//! Because you generally can't know which items are "last" until you've seen
//! the end, `last_n` has to consume the whole source iterator up front. It
//! does this in a single pass using a fixed-size ring buffer (`VecDeque`) of
//! capacity `n`, so memory use is O(n) rather than O(length of iterator).

use std::collections::VecDeque;
use std::iter::FusedIterator;

/// Adds [`last_n`](IteratorExt::last_n) to every `Iterator`.
pub trait IteratorExt: Iterator {
    /// Consumes `self` and returns an iterator over the last `n` items,
    /// in their original relative order.
    ///
    /// - If the source has fewer than `n` items, all of them are yielded.
    /// - If `n == 0`, the source is drained (for its side effects) and
    ///   nothing is yielded.
    /// - This does **not** work on infinite iterators: like `.collect()` or
    ///   `.last()`, it must reach the end of `self` to know what the end is.
    ///
    /// Runs in O(len) time and O(n) space.
    ///
    /// # Example
    /// ```
    /// # use last_n::IteratorExt;
    /// let last_three: Vec<_> = (1..=10).last_n(3).collect();
    /// assert_eq!(last_three, vec![8, 9, 10]);
    /// ```
    fn last_n(self, n: usize) -> LastN<Self::Item>
    where
        Self: Sized,
    {
        let mut buffer: VecDeque<Self::Item> = VecDeque::with_capacity(n);
        if n > 0 {
            for item in self {
                if buffer.len() == n {
                    buffer.pop_front();
                }
                buffer.push_back(item);
            }
        }
        LastN { buffer }
    }
}

// Blanket impl: every Iterator gets `last_n` for free.
impl<I: Iterator> IteratorExt for I {}

/// Iterator over the last `n` items of some source iterator.
///
/// Created by [`IteratorExt::last_n`].
#[derive(Debug, Clone)]
pub struct LastN<T> {
    buffer: VecDeque<T>,
}

impl<T> Iterator for LastN<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.buffer.pop_front()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.buffer.len();
        (len, Some(len))
    }
}

impl<T> DoubleEndedIterator for LastN<T> {
    fn next_back(&mut self) -> Option<T> {
        self.buffer.pop_back()
    }
}

impl<T> ExactSizeIterator for LastN<T> {
    fn len(&self) -> usize {
        self.buffer.len()
    }
}

impl<T> FusedIterator for LastN<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yields_last_n_in_order() {
        let v: Vec<_> = (1..=10).last_n(3).collect();
        assert_eq!(v, vec![8, 9, 10]);
    }

    #[test]
    fn n_larger_than_source_yields_everything() {
        let v: Vec<_> = (1..=3).last_n(10).collect();
        assert_eq!(v, vec![1, 2, 3]);
    }

    #[test]
    fn n_zero_yields_nothing() {
        let v: Vec<i32> = (1..=5).last_n(0).collect();
        assert_eq!(v, Vec::<i32>::new());
    }

    #[test]
    fn empty_source_yields_nothing() {
        let v: Vec<i32> = std::iter::empty().last_n(5).collect();
        assert_eq!(v, Vec::<i32>::new());
    }

    #[test]
    fn works_with_strings_and_non_copy_types() {
        let words = vec!["a", "b", "c", "d", "e"].into_iter().map(String::from);
        let v: Vec<_> = words.last_n(2).collect();
        assert_eq!(v, vec!["d".to_string(), "e".to_string()]);
    }

    #[test]
    fn exact_size_and_double_ended() {
        let mut it = (1..=5).last_n(3);
        assert_eq!(it.len(), 3);
        assert_eq!(it.next_back(), Some(5));
        assert_eq!(it.next(), Some(3));
        assert_eq!(it.next_back(), Some(4));
        assert_eq!(it.next(), None);
        assert_eq!(it.len(), 0);
    }

    #[test]
    fn chains_with_other_adaptors() {
        let v: Vec<_> = (1..100)
            .filter(|x| x % 3 == 0)
            .last_n(4)
            .map(|x| x * 2)
            .collect();
        // multiples of 3 up to 99: ..., 87, 90, 93, 96, 99 -> last 4: 90,93,96,99
        assert_eq!(v, vec![180, 186, 192, 198]);
    }
}
