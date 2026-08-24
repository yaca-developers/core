use std::iter;

use wezterm_term::Line;

pub type LineHash = [u8; 16];

pub struct Scrollback {
    seen: Vec<LineHash>,
}

#[derive(Debug, Default)]
pub struct ScrollbackUpdate {
    pub full_refresh: bool,
    pub new_len: usize,
}

pub trait HashLine {
    fn hash_line(&self) -> LineHash;
}

impl Scrollback {
    pub fn new() -> Self {
        Self { seen: vec![] }
    }

    #[allow(unused)]
    pub fn update<L: HashLine>(&mut self, lines: impl IntoIterator<Item = L>) -> ScrollbackUpdate {
        let mut conclusion = ScrollbackUpdate::default();
        let mut last_seen_line_idx = 0;
        let mut new_lines_iter = lines.into_iter();
        let mut last_new_line = None;
        while last_seen_line_idx < self.seen.len()
            && let Some(new_line) = new_lines_iter.next()
        {
            last_new_line = Some(new_line.hash_line());
            if self.seen[last_seen_line_idx] == last_new_line.unwrap() {
                last_seen_line_idx += 1;
            }
        }

        conclusion.full_refresh = !self.seen.is_empty() && last_seen_line_idx <= 0;

        while self.seen.len() > last_seen_line_idx {
            self.seen.pop();
        }
        if let Some(new_line) = last_new_line {
            self.seen.push(new_line);
            conclusion.new_len = 1;
        }
        while let Some(new_line) = new_lines_iter.next() {
            self.seen.push(new_line.hash_line());
            conclusion.new_len += 1;
        }

        return conclusion;
    }

    pub fn get_unseen<L: HashLine>(
        &self,
        on_screen: impl IntoIterator<Item = L>,
    ) -> impl Iterator<Item = L> {
        let mut seen_iter = self.seen.iter();
        let mut last_tested_line = None;
        let mut on_screen_iter = on_screen.into_iter();
        while let Some(hash) = seen_iter.next()
            && let Some(test_line) = on_screen_iter.next()
            && hash == &test_line.hash_line()
        {
            last_tested_line = Some(test_line);
        }
        iter::once(last_tested_line).flatten().chain(on_screen_iter)
    }
}

impl HashLine for Line {
    fn hash_line(&self) -> LineHash {
        self.compute_shape_hash()
    }
}

impl HashLine for &Line {
    fn hash_line(&self) -> LineHash {
        self.compute_shape_hash()
    }
}
