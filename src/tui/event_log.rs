use std::collections::VecDeque;

pub struct EventLog {
    capacity: usize,
    entries: VecDeque<String>,
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    pub fn push(&mut self, entry: String) {
        if self.capacity == 0 {
            return;
        }

        while self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }

        self.entries.push_back(entry);
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_entries_in_arrival_order() {
        let mut log = EventLog::new(4);

        log.push("first".to_string());
        log.push("second".to_string());

        assert_eq!(log.entries().collect::<Vec<_>>(), vec!["first", "second"]);
    }

    #[test]
    fn discards_the_oldest_entry_once_capacity_is_reached() {
        let mut log = EventLog::new(2);

        log.push("first".to_string());
        log.push("second".to_string());
        log.push("third".to_string());

        assert_eq!(log.entries().collect::<Vec<_>>(), vec!["second", "third"]);
    }
}
