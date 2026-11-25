// SPDX-License-Identifier: GPL-3.0-only

use super::{Mail, MailId, SortOrder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ThreadHeaders {
    pub message_id: Option<String>,
    pub references: Vec<String>,
}

impl ThreadHeaders {
    pub(super) fn from_headers(headers: &mail_parser::Message<'_>) -> Self {
        let references = headers
            .references()
            .into_iter()
            .chain(headers.in_reply_to())
            .flat_map(|ids| ids.iter())
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect();
        Self {
            message_id: headers
                .message_id()
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            references,
        }
    }
}

pub(super) struct Threads {
    groups: Vec<Vec<usize>>,
    oldest_first: Vec<usize>,
    messages: usize,
}

pub(super) struct ThreadSummary<'a> {
    pub key: &'a MailId,
    pub count: usize,
    pub expanded: bool,
    pub unread: bool,
    pub flagged: bool,
}

pub(super) struct ThreadRow<'a> {
    pub index: usize,
    pub mail: &'a Mail,
    pub summary: Option<ThreadSummary<'a>>,
    pub parent: Option<&'a MailId>,
    pub last_reply: bool,
}

impl Threads {
    pub fn new(messages: &[Mail]) -> Self {
        let mut links: Vec<_> = (0..messages.len()).collect();
        let mut ids = HashMap::new();
        for (index, mail) in messages.iter().enumerate() {
            let Some(headers) = &mail.thread else {
                continue;
            };
            for id in headers.message_id.iter().chain(&headers.references) {
                let previous = *ids.entry((mail.id.account, id.as_str())).or_insert(index);
                let left = root(&mut links, previous);
                let right = root(&mut links, index);
                links[left.max(right)] = left.min(right);
            }
        }

        let mut group_indexes = vec![usize::MAX; messages.len()];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for index in 0..messages.len() {
            let group = &mut group_indexes[root(&mut links, index)];
            if *group == usize::MAX {
                *group = groups.len();
                groups.push(Vec::new());
            }
            groups[*group].push(index);
        }
        let mut oldest_first: Vec<_> = (0..groups.len()).collect();
        oldest_first
            .sort_unstable_by_key(|&group| std::cmp::Reverse(*groups[group].last().unwrap()));
        Self {
            groups,
            oldest_first,
            messages: messages.len(),
        }
    }

    pub fn message_count(&self) -> usize {
        self.messages
    }

    pub fn rows<'a>(
        &self,
        messages: &'a [Mail],
        order: SortOrder,
        query: &str,
        expanded: &HashSet<MailId>,
    ) -> Vec<ThreadRow<'a>> {
        let oldest_first = order == SortOrder::OldestFirst;
        let show_all = query.trim().is_empty();
        let mut rows = Vec::with_capacity(self.groups.len());
        for position in 0..self.groups.len() {
            let group = &self.groups[if oldest_first {
                self.oldest_first[position]
            } else {
                position
            }];
            let matches = (0..group.len())
                .map(|position| {
                    group[if oldest_first {
                        group.len() - 1 - position
                    } else {
                        position
                    }]
                })
                .filter(|&index| show_all || messages[index].matches(query));
            let mut members = matches.clone();
            let Some(index) = members.next() else {
                continue;
            };
            let count = 1 + members.clone().count();
            let mail = &messages[index];
            let key = &messages[*group.last().unwrap()].id;
            let is_expanded = expanded.contains(key);
            let summary = (count > 1).then(|| ThreadSummary {
                key,
                count,
                expanded: is_expanded,
                unread: mail.unread || members.clone().any(|index| messages[index].unread),
                flagged: mail.flagged || members.clone().any(|index| messages[index].flagged),
            });
            rows.push(ThreadRow {
                index,
                mail,
                summary,
                parent: None,
                last_reply: false,
            });
            if is_expanded {
                rows.extend(members.enumerate().map(|(position, index)| ThreadRow {
                    index,
                    mail: &messages[index],
                    summary: None,
                    parent: Some(key),
                    last_reply: position + 2 == count,
                }));
            }
        }
        rows
    }
}

fn root(links: &mut [usize], mut index: usize) -> usize {
    while links[index] != index {
        links[index] = links[links[index]];
        index = links[index];
    }
    index
}
