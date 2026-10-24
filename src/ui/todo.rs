//! To-do list: today's tasks with a done counter, inline text entry and an
//! edit mode for renaming/deleting. Tasks persist in `todo.json`; tasks
//! completed on previous days are cleared at the start of a new day.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use vello::kurbo::{Circle, Point, Rect};

use super::common::{ButtonStyle, capsule_button, mix, status_bar};
use super::{HAlign, Hit, Mode, Model, Painter, VAlign, page_navigation, palette, rgba};
use crate::renderer::icons::Icon;
use crate::renderer::shapes::capsule;
use crate::renderer::text::TextStyle;

const COUNT: TextStyle = TextStyle::new(32.0, TextStyle::BOLD).tabular();
const LABEL: TextStyle = TextStyle::new(13.0, TextStyle::REGULAR);
const BUTTON: TextStyle = TextStyle::new(12.5, TextStyle::MEDIUM);
const TASK: TextStyle = TextStyle::new(13.0, TextStyle::MEDIUM);
pub const LIST: Rect = Rect::new(232.0, 38.0, 398.0, 152.0);
const ROW_HEIGHT: f64 = 30.0;
const ROW_GAP: f64 = 6.0;
const MAX_TEXT: usize = 120;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub text: String,
    pub done: bool,
    #[serde(default)]
    pub completed_on: Option<NaiveDate>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoData {
    pub tasks: Vec<Task>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    New,
    Rename(u64),
}

/// Single-line text field state.
#[derive(Clone, Debug)]
pub struct TextInput {
    pub text: String,
    /// Byte index of the caret.
    pub cursor: usize,
    pub target: EditTarget,
}

impl TextInput {
    pub fn new(target: EditTarget, text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor, target }
    }

    pub fn insert(&mut self, input: &str) {
        let clean: String = input.chars().filter(|c| !c.is_control()).collect();
        let room = MAX_TEXT.saturating_sub(self.text.chars().count());
        let clean: String = clean.chars().take(room).collect();
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    pub fn backspace(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.replace_range(index..self.cursor, "");
            self.cursor = index;
        }
    }

    pub fn delete(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text.replace_range(self.cursor..self.cursor + c.len_utf8(), "");
        }
    }

    pub fn left(&mut self) {
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }
}

pub struct TodoList {
    pub data: TodoData,
    pub editing: Option<TextInput>,
    pub edit_mode: bool,
}

impl TodoList {
    pub fn new(data: TodoData) -> Self {
        Self { data, editing: None, edit_mode: false }
    }

    /// (done today, tasks for today).
    pub fn counts(&self, today: NaiveDate) -> (usize, usize) {
        let relevant = self.data.tasks.iter().filter(|t| !t.done || t.completed_on == Some(today));
        let (mut done, mut total) = (0, 0);
        for task in relevant {
            total += 1;
            done += task.done as usize;
        }
        (done, total)
    }

    pub fn toggle(&mut self, id: u64, today: NaiveDate) {
        if let Some(task) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            task.done = !task.done;
            task.completed_on = task.done.then_some(today);
        }
    }

    pub fn delete(&mut self, id: u64) {
        self.data.tasks.retain(|t| t.id != id);
        if self.editing.as_ref().is_some_and(|e| e.target == EditTarget::Rename(id)) {
            self.editing = None;
        }
    }

    pub fn begin_add(&mut self) {
        self.editing = Some(TextInput::new(EditTarget::New, String::new()));
    }

    pub fn begin_rename(&mut self, id: u64) {
        if let Some(task) = self.data.tasks.iter().find(|t| t.id == id) {
            self.editing = Some(TextInput::new(EditTarget::Rename(id), task.text.clone()));
        }
    }

    /// Applies the text field. Returns `true` when the data changed.
    pub fn commit(&mut self) -> bool {
        let Some(input) = self.editing.take() else { return false };
        let text = input.text.trim().to_string();
        match input.target {
            EditTarget::New if !text.is_empty() => {
                let id = self.data.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
                self.data.tasks.insert(0, Task { id, text, done: false, completed_on: None });
                true
            }
            EditTarget::Rename(id) if !text.is_empty() => {
                match self.data.tasks.iter_mut().find(|t| t.id == id) {
                    Some(task) if task.text != text => {
                        task.text = text;
                        true
                    }
                    _ => false,
                }
            }
            EditTarget::Rename(id) => {
                // Clearing the name deletes the task.
                self.delete(id);
                true
            }
            EditTarget::New => false,
        }
    }

    pub fn cancel(&mut self) {
        self.editing = None;
    }

    /// Drops tasks completed before `today`. Returns `true` when changed.
    pub fn rollover(&mut self, today: NaiveDate) -> bool {
        let before = self.data.tasks.len();
        self.data.tasks.retain(|t| !t.done || t.completed_on.is_none_or(|day| day >= today));
        self.data.tasks.len() != before
    }

    pub fn content_height(&self) -> f64 {
        let rows = self.data.tasks.len() + usize::from(matches!(self.editing, Some(TextInput { target: EditTarget::New, .. })));
        (rows as f64 * (ROW_HEIGHT + ROW_GAP) - ROW_GAP).max(0.0)
    }

    pub fn max_scroll(&self) -> f64 {
        (self.content_height() - LIST.height()).max(0.0)
    }
}

pub fn draw(p: &mut Painter, m: &Model) {
    let width = Mode::Todo.size().width;
    let todo = m.todo;
    status_bar(p, m, width, &[("Todo", palette::WHITE), ("List", palette::GREEN)]);
    page_navigation(p, m, width, 86.0);

    let (done, total) = todo.counts(m.clock.date_naive());
    p.text(&format!("{done}/{total}"), &COUNT, Point::new(52.0, 62.0), HAlign::Left, VAlign::Center, palette::WHITE);
    p.text("Task Done Today", &LABEL, Point::new(52.0, 92.0), HAlign::Left, VAlign::Center, palette::GRAY);
    capsule_button(
        p,
        Hit::TodoAdd,
        Rect::new(48.0, 118.0, 134.0, 144.0),
        Some(Icon::PlusCircle),
        "Add Task",
        ButtonStyle::GRAY,
        &BUTTON,
    );
    let (edit_icon, edit_label) = if todo.edit_mode { (Icon::Check, "Done") } else { (Icon::Pencil, "Edit Task") };
    capsule_button(
        p,
        Hit::TodoEdit,
        Rect::new(138.0, 118.0, 222.0, 144.0),
        Some(edit_icon),
        edit_label,
        ButtonStyle::GRAY,
        &BUTTON,
    );
    list(p, m);
    super::page_dots(p, m, width);
}

fn list(p: &mut Painter, m: &Model) {
    let todo = m.todo;
    p.hit(Hit::TodoList, LIST);
    let adding = matches!(todo.editing, Some(TextInput { target: EditTarget::New, .. }));
    if todo.data.tasks.is_empty() && !adding {
        p.text("No tasks yet", &LABEL, Point::new(LIST.center().x, 84.0), HAlign::Center, VAlign::Center, palette::GRAY);
        return;
    }
    let scroll = m.todo_scroll.clamp(0.0, todo.max_scroll());
    p.push_clip(&Rect::new(LIST.x0 - 2.0, LIST.y0, LIST.x1 + 12.0, LIST.y1));
    let mut y = LIST.y0 - scroll;
    if let (true, Some(input)) = (adding, &todo.editing) {
        input_row(p, m, input, Rect::new(LIST.x0, y, LIST.x1, y + ROW_HEIGHT));
        y += ROW_HEIGHT + ROW_GAP;
    }
    for task in &todo.data.tasks {
        let row = Rect::new(LIST.x0, y, LIST.x1, y + ROW_HEIGHT);
        y += ROW_HEIGHT + ROW_GAP;
        if row.y1 < LIST.y0 || row.y0 > LIST.y1 {
            continue;
        }
        match &todo.editing {
            Some(input) if input.target == EditTarget::Rename(task.id) => input_row(p, m, input, row),
            _ => task_row(p, todo.edit_mode, task, row),
        }
    }
    p.pop_clip();

    // Thin scrollbar when the list overflows.
    let content = todo.content_height();
    if content > LIST.height() + 0.5 {
        let track = Rect::new(LIST.x1 + 5.0, LIST.y0 + 2.0, LIST.x1 + 7.5, LIST.y1 - 2.0);
        p.fill(&capsule(track), rgba(0xFFFFFF, 0.10));
        let thumb_h = (track.height() * LIST.height() / content).max(16.0);
        let travel = track.height() - thumb_h;
        let top = track.y0 + travel * (scroll / todo.max_scroll().max(1.0));
        p.fill(&capsule(Rect::new(track.x0, top, track.x1, top + thumb_h)), rgba(0xFFFFFF, 0.6));
    }
}

fn task_row(p: &mut Painter, edit_mode: bool, task: &Task, row: Rect) {
    let hover = p.hover(Hit::TodoText(task.id)).max(p.hover(Hit::TodoCheck(task.id)));
    let base = if task.done { rgba(0x34C759, 0.20) } else { rgba(0x34C759, 0.11) };
    p.fill(&capsule(row), mix(base, rgba(0x34C759, 0.28), hover * 0.6));
    let check = Point::new(row.x0 + 16.0, row.center().y);
    if task.done {
        p.fill(&Circle::new(check, 8.0), palette::GREEN);
        p.icon(Icon::Check, check, 11.5, palette::WHITE);
    } else {
        p.stroke(&Circle::new(check, 7.2), 1.6, palette::GREEN);
    }
    p.hit(Hit::TodoCheck(task.id), Rect::from_center_size(check, (26.0, ROW_HEIGHT)));

    let text_x = row.x0 + 32.0;
    let text_w = (row.x1 - text_x - if edit_mode { 30.0 } else { 10.0 }) as f32;
    let color = if task.done { rgba(0xFFFFFF, 0.62) } else { palette::WHITE };
    let rect = p.text(&task.text, &TASK.max_width(text_w), Point::new(text_x, row.center().y), HAlign::Left, VAlign::Center, color);
    if task.done {
        let y = row.center().y + 0.5;
        p.fill(&Rect::new(rect.x0 - 1.0, y - 0.6, rect.x1 + 1.0, y + 0.6), rgba(0xFFFFFF, 0.7));
    }
    p.hit(Hit::TodoText(task.id), Rect::new(text_x - 4.0, row.y0, row.x1 - if edit_mode { 28.0 } else { 0.0 }, row.y1));
    if edit_mode {
        let center = Point::new(row.x1 - 14.0, row.center().y);
        let hover = p.hover(Hit::TodoDelete(task.id));
        p.fill(&Circle::new(center, 9.0), mix(rgba(0xFF453A, 0.25), rgba(0xFF453A, 0.55), hover));
        p.icon(Icon::Minus, center, 11.0, palette::WHITE);
        p.hit(Hit::TodoDelete(task.id), Rect::from_center_size(center, (22.0, 22.0)));
    }
}

fn input_row(p: &mut Painter, m: &Model, input: &TextInput, row: Rect) {
    p.fill(&capsule(row), rgba(0xFFFFFF, 0.12));
    p.stroke(&capsule(row.inflate(-0.5, -0.5)), 1.0, rgba(0x34C759, 0.7));
    let text_x = row.x0 + 12.0;
    let cy = row.center().y;
    let available = row.x1 - text_x - 12.0;
    let prefix_w = p.measure(&input.text[..input.cursor], &TASK).width;
    // Keep the caret visible for long input by shifting the text left.
    let shift = (prefix_w - available + 4.0).max(0.0);
    p.push_clip(&capsule(row.inflate(-3.0, 0.0)));
    if input.text.is_empty() {
        p.text("New task\u{2026}", &TASK, Point::new(text_x, cy), HAlign::Left, VAlign::Center, rgba(0xFFFFFF, 0.38));
    } else {
        p.text(&input.text, &TASK, Point::new(text_x - shift, cy), HAlign::Left, VAlign::Center, palette::WHITE);
    }
    if m.caret_visible {
        let x = text_x - shift + prefix_w + 0.5;
        p.fill(&Rect::new(x, cy - 8.0, x + 1.5, cy + 8.0), palette::GREEN);
    }
    p.pop_clip();
    p.hit(Hit::TodoInput, row);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[test]
    fn add_toggle_rollover() {
        let mut list = TodoList::new(TodoData::default());
        list.begin_add();
        list.editing.as_mut().unwrap().insert("Revise the report");
        assert!(list.commit());
        list.begin_add();
        list.editing.as_mut().unwrap().insert("   ");
        assert!(!list.commit());
        let id = list.data.tasks[0].id;
        list.toggle(id, day(24));
        assert_eq!(list.counts(day(24)), (1, 1));
        assert!(list.rollover(day(25)));
        assert!(list.data.tasks.is_empty());
    }

    #[test]
    fn text_input_editing() {
        let mut input = TextInput::new(EditTarget::New, "héllo".into());
        input.left();
        input.backspace();
        assert_eq!(input.text, "hélo");
        input.home();
        input.delete();
        assert_eq!(input.text, "élo");
        input.end();
        input.insert("!\n");
        assert_eq!(input.text, "élo!");
    }
}
