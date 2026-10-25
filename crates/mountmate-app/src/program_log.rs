//! Bounded, in-memory application feedback. Only explicit user-facing messages
//! enter this log; never record Debug output of commands, forms, or SAI responses.
use std::collections::VecDeque;

use iced::widget::{
    Space, button, checkbox, column, container, pane_grid, row, text, text_editor, text_input,
};
use iced::{Center, Element, Fill, Font, Task, Theme, clipboard};

use crate::i18n::Locale;

const MAX_ENTRIES: usize = 500;
const MAX_MESSAGE_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    fn label(self, locale: Locale) -> &'static str {
        match (self, locale) {
            (Self::Info, Locale::English) => "INFO",
            (Self::Warning, Locale::English) => "WARNING",
            (Self::Error, Locale::English) => "ERROR",
            (Self::Info, Locale::Chinese) => "信息",
            (Self::Warning, Locale::Chinese) => "注意",
            (Self::Error, Locale::Chinese) => "错误",
        }
    }

    pub fn style(self, theme: &Theme) -> container::Style {
        let palette = theme.extended_palette();
        let pair = match self {
            Self::Info => palette.primary.weak,
            Self::Warning => palette.warning.weak,
            Self::Error => palette.danger.weak,
        };
        container::Style::default()
            .background(pair.color)
            .color(pair.text)
            .border(iced::Border::default().rounded(5))
    }
}

struct Entry {
    time: String,
    level: Level,
    message: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Resize(pane_grid::ResizeEvent),
    Toggle,
    Filter(String),
    Follow(bool),
    Copy,
    Clear,
    Action(text_editor::Action),
}

pub struct ProgramLog {
    /// false = application screen; true = program log.
    pub panes: pane_grid::State<bool>,
    pub collapsed: bool,
    entries: VecDeque<Entry>,
    content: text_editor::Content,
    filter: String,
    follow: bool,
    pending: usize,
    locale: Locale,
}

impl ProgramLog {
    pub fn new(locale: Locale) -> Self {
        Self {
            panes: pane_grid::State::with_configuration(pane_grid::Configuration::Split {
                axis: pane_grid::Axis::Horizontal,
                ratio: 0.70,
                a: Box::new(pane_grid::Configuration::Pane(false)),
                b: Box::new(pane_grid::Configuration::Pane(true)),
            }),
            collapsed: false,
            entries: VecDeque::new(),
            content: text_editor::Content::new(),
            filter: String::new(),
            follow: true,
            pending: 0,
            locale,
        }
    }

    pub fn info(&mut self, message: String) -> String {
        self.publish(Level::Info, message)
    }

    pub fn warning(&mut self, message: String) -> String {
        self.publish(Level::Warning, message)
    }

    pub fn error(&mut self, message: String) -> String {
        self.publish(Level::Error, message)
    }

    /// Return the original status for existing UI/refresh guards. History is
    /// bounded independently, including messages supplied by external tools.
    pub fn publish(&mut self, level: Level, message: String) -> String {
        if message.trim().is_empty() {
            return message;
        }
        let mut bounded: String = message.chars().take(MAX_MESSAGE_CHARS).collect();
        if bounded.len() < message.len() {
            bounded.push('…');
        }
        if level != Level::Info {
            self.collapsed = false;
        }
        if self
            .entries
            .back()
            .is_some_and(|last| last.level == level && last.message == bounded)
        {
            return message;
        }
        self.entries.push_back(Entry {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            message: bounded,
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
        if self.follow {
            self.refresh();
        } else {
            self.pending = self.pending.saturating_add(1);
        }
        message
    }

    pub fn set_locale(&mut self, locale: Locale) {
        if self.locale != locale {
            self.locale = locale;
            self.refresh();
        }
    }

    fn refresh(&mut self) {
        let query = self.filter.to_lowercase();
        let lines = self
            .entries
            .iter()
            .filter_map(|entry| {
                let line = format!(
                    "[{}] [{}] {}",
                    entry.time,
                    entry.level.label(self.locale),
                    entry.message
                );
                line.to_lowercase().contains(&query).then_some(line)
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.content = text_editor::Content::with_text(&lines);
        if self.follow {
            self.content
                .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        }
        self.pending = 0;
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Resize(event) => {
                self.panes
                    .resize(event.split, event.ratio.clamp(0.35, 0.80));
                if self.follow {
                    self.content
                        .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
                }
            }
            Message::Toggle => self.collapsed = !self.collapsed,
            Message::Filter(value) => {
                self.filter = value;
                self.refresh();
            }
            Message::Follow(follow) => {
                self.follow = follow;
                if follow {
                    self.refresh();
                }
            }
            Message::Copy => {
                return clipboard::write(
                    self.content
                        .selection()
                        .filter(|selection| !selection.is_empty())
                        .unwrap_or_else(|| self.content.text()),
                );
            }
            Message::Clear => {
                self.entries.clear();
                self.refresh();
            }
            Message::Action(action) if !action.is_edit() => {
                // Keep scroll position and selection stable while reading history.
                self.follow = false;
                self.content.perform(action);
            }
            Message::Action(_) => {}
        }
        Task::none()
    }

    pub fn view(&self, locale: Locale) -> Element<'_, Message> {
        let tr = |en, zh| if locale == Locale::English { en } else { zh };
        let title = text(format!(
            "{} ({}/{MAX_ENTRIES})",
            tr("Program log", "程序日志"),
            self.entries.len()
        ))
        .size(16);
        let toggle = button(tr(
            if self.collapsed { "Expand" } else { "Collapse" },
            if self.collapsed { "展开" } else { "收起" },
        ))
        .on_press(Message::Toggle);
        let mut header = row![title].spacing(10).align_y(Center);
        if !self.collapsed {
            header = header
                .push(
                    text_input(tr("Filter messages", "筛选日志"), &self.filter)
                        .on_input(Message::Filter)
                        .width(180)
                        .size(14),
                )
                .push(
                    checkbox(self.follow)
                        .label(tr("Follow", "实时滚动"))
                        .on_toggle(Message::Follow)
                        .size(16)
                        .text_size(14),
                )
                .push(button(tr("Copy", "复制")).on_press(Message::Copy))
                .push(button(tr("Clear", "清空")).on_press(Message::Clear));
            if self.pending > 0 {
                header = header.push(text(format!("+{}", self.pending)).size(14));
            }
        }
        header = header.push(Space::new().width(Fill)).push(toggle);
        let mut body = column![header].spacing(8);
        if let Some(entry) = self.entries.back() {
            // The latest feedback stays visible even when history is filtered,
            // paused, or collapsed. Full text remains copyable in the log.
            let summary: String = entry
                .message
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(180)
                .collect();
            body = body.push(
                container(
                    text(format!(
                        "[{}] [{}] {}",
                        entry.time,
                        entry.level.label(locale),
                        summary
                    ))
                    .size(15),
                )
                .padding([7, 10])
                .width(Fill)
                .style(move |theme| entry.level.style(theme)),
            );
        }
        if !self.collapsed {
            body = body.push(
                text_editor(&self.content)
                    .placeholder(tr(
                        "Application messages appear here",
                        "程序运行消息会显示在这里",
                    ))
                    .on_action(Message::Action)
                    .font(Font::MONOSPACE)
                    .size(14)
                    .height(Fill),
            );
        }
        container(body).padding([10, 18]).width(Fill).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_history_retains_new_errors_without_losing_selection() {
        let mut log = ProgramLog::new(Locale::English);
        log.info("Mounted".into());
        let _ = log.update(Message::Action(text_editor::Action::SelectAll));
        let selection = log.content.selection();
        log.collapsed = true;
        log.error("Upload failed".into());
        assert!(!log.collapsed);
        assert_eq!(log.content.selection(), selection);
        assert_eq!(log.pending, 1);
        let _ = log.update(Message::Follow(true));
        assert!(log.content.text().contains("Upload failed"));
        assert_eq!(log.pending, 0);
        log.collapsed = true;
        log.error("Upload failed".into());
        assert!(!log.collapsed);
        assert_eq!(log.entries.len(), 2);
    }

    #[test]
    fn filtering_is_reversible_and_clear_removes_all_history() {
        let mut log = ProgramLog::new(Locale::English);
        log.info("Mounted".into());
        log.error("SAI unavailable".into());
        let _ = log.update(Message::Filter("sai".into()));
        assert!(!log.content.text().contains("Mounted"));
        assert!(log.content.text().contains("SAI unavailable"));
        let _ = log.update(Message::Filter(String::new()));
        assert!(log.content.text().contains("Mounted"));
        let _ = log.update(Message::Clear);
        let _ = log.update(Message::Filter("SAI".into()));
        assert!(log.content.is_empty());
        assert!(log.entries.is_empty());
    }

    #[test]
    fn history_is_bounded_and_read_only() {
        let mut log = ProgramLog::new(Locale::Chinese);
        for i in 0..MAX_ENTRIES + 10 {
            log.info(format!("消息 {i}"));
        }
        assert_eq!(log.entries.len(), MAX_ENTRIES);
        assert_eq!(log.entries.front().unwrap().message, "消息 10");
        let full = "字".repeat(MAX_MESSAGE_CHARS + 100);
        assert_eq!(log.error(full.clone()), full);
        assert_eq!(
            log.entries.back().unwrap().message.chars().count(),
            MAX_MESSAGE_CHARS + 1
        );
        let original = log.content.text();
        let _ = log.update(Message::Action(text_editor::Action::Edit(
            text_editor::Edit::Insert('X'),
        )));
        assert_eq!(log.content.text(), original);
    }
}
