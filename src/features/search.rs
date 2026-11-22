// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use cosmic::{Element, Task, widget};

#[derive(Clone, Debug)]
pub enum Message {
    Changed(String),
    Focus,
    Submit,
}

pub struct Search {
    query: String,
    id: widget::Id,
}

impl Search {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            id: widget::Id::unique(),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
    }

    pub fn clear(&mut self) {
        self.query.clear();
    }

    pub fn focus(&self) -> Task<Message> {
        widget::text_input::focus(self.id.clone())
    }

    pub fn view(&self, narrow: bool) -> Element<'_, Message> {
        widget::search_input(fl!("search-mail"), self.query.as_str())
            .id(self.id.clone())
            .width(if narrow { 155 } else { 280 })
            .on_input(Message::Changed)
            .on_submit(|_| Message::Submit)
            .into()
    }
}
