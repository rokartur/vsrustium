//! `HistoryNavigator` (base/common/history.ts) over an insertion-ordered set of strings, with the navigation
//! `HistoryInputBox` (base/browser/ui/inputbox/inputBox.ts) runs on it for Up and Down.

pub struct HistoryNavigator {
    elements: Vec<String>,
    limit: usize,
    /// `ArrayNavigator.index`: -1 before the first element, `elements.len()` past the last.
    index: isize,
}

impl HistoryNavigator {
    pub fn new(elements: Vec<String>, limit: usize) -> Self {
        let mut navigator = HistoryNavigator { elements, limit, index: 0 };
        navigator.on_change();
        navigator
    }

    pub fn elements(&self) -> &[String] {
        &self.elements
    }

    pub fn add(&mut self, value: &str) {
        self.elements.retain(|element| element != value);
        self.elements.push(value.to_string());
        self.on_change();
    }

    pub fn next(&mut self) -> Option<String> {
        self.index = (self.index + 1).min(self.elements.len() as isize);
        self.current()
    }

    pub fn previous(&mut self) -> Option<String> {
        if self.current_position() == Some(0) {
            return None;
        }
        self.index = (self.index - 1).max(-1);
        self.current()
    }

    pub fn current(&self) -> Option<String> {
        usize::try_from(self.index).ok().and_then(|index| self.elements.get(index)).cloned()
    }

    pub fn first(&mut self) -> Option<String> {
        self.index = 0;
        self.current()
    }

    pub fn last(&mut self) -> Option<String> {
        self.index = self.elements.len() as isize - 1;
        self.current()
    }

    #[cfg(test)]
    pub fn is_first(&self) -> bool {
        self.current_position() == Some(0)
    }

    pub fn is_last(&self) -> bool {
        self.current_position().is_some_and(|position| position + 1 >= self.elements.len())
    }

    pub fn is_nowhere(&self) -> bool {
        self.current().is_none()
    }

    pub fn has(&self, value: &str) -> bool {
        self.elements.iter().any(|element| element == value)
    }

    pub fn clear(&mut self) {
        self.elements.clear();
        self.on_change();
    }

    /// `HistoryInputBox.addToHistory`: the value unless it is empty or, without `always`, the current one.
    pub fn add_to_history(&mut self, value: &str, always: bool) {
        if !value.is_empty() && (always || self.current_value().as_deref() != Some(value)) {
            self.add(value);
        }
    }

    /// `HistoryInputBox.showNextValue`: the input's new value, empty past the last element.
    pub fn show_next_value(&mut self, value: &str) -> String {
        if !self.has(value) {
            self.add_to_history(value, false);
        }
        let mut next = self.next();
        if next.as_deref() == Some(value) {
            next = self.next();
        }
        next.unwrap_or_default()
    }

    /// `HistoryInputBox.showPreviousValue`: the input's new value, `None` to keep it.
    pub fn show_previous_value(&mut self, value: &str) -> Option<String> {
        if !self.has(value) {
            self.add_to_history(value, false);
        }
        let mut previous = self.previous().or_else(|| self.first());
        if previous.as_deref() == Some(value) {
            previous = self.previous().or_else(|| self.first());
        }
        previous.filter(|previous| !previous.is_empty())
    }

    /// `HistoryInputBox.getCurrentValue`: nowhere in the history counts as the last element, and steps past it.
    fn current_value(&mut self) -> Option<String> {
        let current = self.current();
        if current.is_some() {
            return current;
        }
        let last = self.last();
        self.next();
        last
    }

    /// `_onChange`: drops the oldest elements past the limit and moves past the last.
    fn on_change(&mut self) {
        let excess = self.elements.len().saturating_sub(self.limit);
        self.elements.drain(..excess);
        self.index = self.elements.len() as isize;
    }

    fn current_position(&self) -> Option<usize> {
        usize::try_from(self.index).ok().filter(|&index| index < self.elements.len())
    }
}

#[cfg(test)]
mod tests;
