use chrono::{DateTime, Utc};
use rememberthemilk::{Perms, API, RTMTasks, RTMList, TaskSeries};
use tokio_stream::StreamExt;
use tui::{
    backend::CrosstermBackend,
    widgets::{List, Block, Borders, BorderType, ListItem, ListState, Paragraph, Clear},
    Terminal, style::{Style, Color, Modifier}, text::{Spans, Span}, layout::Rect
};
use crossterm::{terminal::{disable_raw_mode, enable_raw_mode}, event::{KeyCode, Event, EventStream}};
use std::io;

use crate::{get_rtm_api, get_default_filter, tail_end};

enum DisplayMode {
    Tasks,
    Lists,
}

struct ListDispState {
    list: RTMList,
    opened: bool,
    tasks: Option<RTMTasks>,
}

struct UiState {
    display_mode: DisplayMode,
    filter: String,
    list_state: ListState,
    list_pos: usize,
    list_items: Vec<ListItem<'static>>,
    list_paths: Vec<(usize, usize)>,
    tasks: RTMTasks,
    lists: Vec<ListDispState>,
    show_task: bool,
    input_prompt: &'static str,
    input_value: String,
    show_input: bool,
}

struct RtmTaskListIterator<'t> {
    tasks: &'t RTMTasks,
    next_list_idx: Option<usize>,
    next_task_idx: usize,
}

impl<'t> RtmTaskListIterator<'t> {
    pub fn new(tasks: &'t RTMTasks) -> Self {
        RtmTaskListIterator {
            tasks,
            next_list_idx: Some(0),
            next_task_idx: 0,
        }
    }
}

impl<'t> Iterator for RtmTaskListIterator<'t> {
    type Item = &'t TaskSeries;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let list_idx = match self.next_list_idx {
                Some(idx) => idx,
                None => { return None; }
            };
            if list_idx >= self.tasks.list.len() {
                // We're done.
                self.next_list_idx = None;
                return None
            }
            let list = &self.tasks.list[list_idx];
            if let Some(vseries) = list.taskseries.as_ref() {
                if self.next_task_idx >= vseries.len() {
                    // Try the next list
                    self.next_list_idx = Some(list_idx + 1);
                    self.next_task_idx = 0;
                    continue;
                }
                let idx = self.next_task_idx;
                self.next_task_idx += 1;
                return Some(&vseries[idx])
            } else {
                // No taskseries
                self.next_list_idx  = Some(list_idx + 1);
                self.next_task_idx = 0;
                continue;
            }
        }
    }
}

struct Tui {
    api: API,
    events: EventStream,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: UiState,
}
enum StepResult {
    Cont,
    End,
}
impl Tui {
    pub async fn new() -> Result<Tui, failure::Error> {
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let events = crossterm::event::EventStream::new();

        let api = get_rtm_api(Perms::Read).await?;
        let list_state: ListState = Default::default();
        let filter = get_default_filter()?;
        let show_task = false;
        let display_mode = DisplayMode::Tasks;

        let ui_state = UiState {
            display_mode,
            filter,
            list_state,
            list_pos: 0,
            list_items: vec![],
            list_paths: vec![],
            tasks: Default::default(),
            lists: Default::default(),
            show_task,
            input_prompt: "",
            input_value: String::new(),
            show_input: false,
        };

        let mut tui = Tui {
            api,
            events,
            terminal,
            ui_state,
        };
        tui.update_tasks().await?;

        Ok(tui)
    }
    async fn update_tasks(&mut self) -> Result<(), failure::Error> {
        let tasks = self.api.get_tasks_filtered(&self.ui_state.filter).await?;
        let list_pos = 0;
        self.ui_state.list_state.select(Some(list_pos));

        let mut list_items = vec![];
        let mut list_paths = vec![];
        for (ti, ts) in RtmTaskListIterator::new(&tasks).enumerate() {
            list_paths.push((ti, 0));
            list_items.push(ListItem::new(ts.name.clone()));
        }
        if list_items.is_empty() {
            list_items.push(ListItem::new("[No tasks in current list]"));
        }
        self.ui_state.tasks = tasks;
        self.ui_state.list_items = list_items;
        self.ui_state.list_paths = list_paths;
        self.ui_state.list_pos = list_pos;
        self.ui_state.display_mode = DisplayMode::Tasks;
        Ok(())
    }

    async fn update_list_display(&mut self) -> Result<(), failure::Error> {
        let mut list_items = vec![];
        let mut list_paths = vec![];
        for (i, list) in self.ui_state.lists.iter().enumerate() {
            list_items.push(ListItem::new(list.list.name.clone()));
            list_paths.push((i, 0));
            if list.opened {
                if let Some(tasks) = list.tasks.as_ref() {
                    for (ti, task) in RtmTaskListIterator::new(tasks).enumerate() {
                        list_paths.push((i, ti+1));
                        list_items.push(ListItem::new(format!("  {}", task.name)));
                    }
                }
            }
        }
        if list_items.is_empty() {
            list_items.push(ListItem::new("[No lists]"));
        }
        self.ui_state.list_items = list_items;
        self.ui_state.list_paths = list_paths;
        self.ui_state.display_mode = DisplayMode::Lists;
        Ok(())
    }

    async fn update_lists(&mut self) -> Result<(), failure::Error> {
        let lists = self.api.get_lists().await?;
        self.ui_state.list_state.select(Some(0));

        self.ui_state.lists = lists.into_iter().map(|l| ListDispState {
            list: l,
            opened: false,
            tasks: None,
        }).collect();
        self.ui_state.list_pos = 0;
        self.ui_state.show_task = false;
        self.update_list_display().await
    }

    async fn draw(&mut self) -> Result<(), failure::Error> {
        let ui_state = &mut self.ui_state;
        self.terminal.draw(move |f| {
            let size = f.size();
            let block = Block::default()
                .title("RTM list")
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black));
            let list = List::new(&ui_state.list_items[..])
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("*");
            let mut list_size = size;
            if ui_state.show_task {
                list_size.height = list_size.height / 2;
            }
            f.render_stateful_widget(list, list_size, &mut ui_state.list_state);

            if ui_state.show_task && !ui_state.list_paths.is_empty() {
                let block = Block::default()
                    .title("Task")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::White))
                    .border_type(BorderType::Rounded)
                    .style(Style::default().bg(Color::Black));
                let (li, _) = ui_state.list_paths[ui_state.list_pos];
                let series = RtmTaskListIterator::new(&ui_state.tasks).nth(li).unwrap();
                let mut text = vec![
                    Spans::from(vec![
                        Span::raw(series.name.clone()),
                    ])];
                if !series.tags.is_empty() {
                    let mut spans = vec![
                        Span::raw("Tags: ")];
                    let tag_style = Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD);
                    for tag in &series.tags {
                        spans.push(Span::styled(tag.clone(), tag_style));
                        spans.push(" ".into());
                    }
                    text.push( Spans::from(spans));
                }
                if let Some(repeat) = &series.repeat {
                    let style = Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD);
                    let mut spans = vec![
                        Span::raw("Repeat: ")];
                    if repeat.every {
                        spans.push(Span::raw("every "));
                    } else {
                        spans.push(Span::raw("after "));
                    }
                    spans.push(
                        Span::styled(repeat.rule.clone(), style));
                    text.push( Spans::from(spans));
                }
                for task in &series.task {
                    fn add_date_field(text: &mut Vec<Spans>, heading: &'static str,
                                      value: &Option<DateTime<Utc>>,
                                      color: Color) {
                        if let Some(date) = value {
                            let style = Style::default()
                                .fg(color)
                                .add_modifier(Modifier::BOLD);
                            let mut spans = vec![
                                Span::raw(heading)];
                            spans.push(
                                Span::styled(format!("{}", date), style));
                            text.push( Spans::from(spans));
                        }
                    }
                    add_date_field(&mut text, "Due: ", &task.due, Color::Yellow);
                    add_date_field(&mut text, "Completed: ", &task.completed, Color::Magenta);
                    add_date_field(&mut text, "Deleted: ", &task.deleted, Color::Red);
                }
                let par = Paragraph::new(text)
                    .block(block);
                let area = Rect::new(
                    0, list_size.height,
                    size.width, size.height - list_size.height);
                f.render_widget(par, area);
            }
            if ui_state.show_input {
                let block = Block::default()
                    .title(ui_state.input_prompt)
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::White))
                    .style(Style::default().bg(Color::Black));
                let area = Rect::new(0, size.height-2, size.width, 2);
                f.render_widget(Clear, area);

                let visible_value = tail_end(&ui_state.input_value, size.width as usize -1);
                let text = vec![
                    Span::raw(visible_value),
                    Span::raw("_"),
                ];
                f.render_widget(
                    Paragraph::new(vec![Spans::from(text)])
                        .block(block), area);
            }
        })?;
        Ok(())
    }

    async fn input(&mut self, prompt: &'static str) -> Result<String, failure::Error> {
        self.ui_state.input_value = self.ui_state.filter.clone();
        self.ui_state.input_prompt = prompt;
        self.ui_state.show_input = true;
        loop {
            self.draw().await?;
            match self.events.next().await {
                None => { return Ok("".into()); }
                Some(ev) => match ev {
                    Err(e) => { return Err(e.into()); }
                    Ok(ev) => match ev {
                        Event::Key(key) => {
                            match key.code {
                                KeyCode::Char(c) => {
                                    self.ui_state.input_value.push(c);
                                }
                                KeyCode::Enter => {
                                    break;
                                }
                                KeyCode::Backspace => {
                                    let _ = self.ui_state.input_value.pop();
                                }
                                KeyCode::Esc => {
                                    self.ui_state.show_input = false;
                                    return Ok(String::new());
                                }
                                _ => (),
                            }
                        }
                        _ => ()
                    }
                }
            }
        }
        self.ui_state.show_input = false;

        let mut result = String::new();
        std::mem::swap(&mut result, &mut self.ui_state.input_value);
        Ok(result)
    }

    pub async fn step(&mut self) -> Result<StepResult, failure::Error> {
        self.draw().await?;

        let result = match self.events.next().await {
            None => { return Ok(StepResult::End); }
            Some(ev) => match ev {
                Err(e) => { return Err(e.into()); }
                Ok(ev) => match ev {
                    Event::Key(key) => {
                        match key.code {
                            KeyCode::Char('q') => {
                                StepResult::End
                            }
                            KeyCode::Char('g') => {
                                let filter = self.input("Enter RTM filter:").await?;
                                if !filter.is_empty() {
                                    self.ui_state.filter = filter;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            KeyCode::Char('L') => {
                                self.update_lists().await?;
                                StepResult::Cont
                            }
                            KeyCode::Enter => {
                                match self.ui_state.display_mode {
                                    DisplayMode::Tasks => {
                                        self.ui_state.show_task = !self.ui_state.show_task;
                                    }
                                    DisplayMode::Lists => {
                                        // Expand/unexpand the list
                                        if self.ui_state.list_items.len() > 0 {
                                            let (li, ti) = self.ui_state.list_paths[self.ui_state.list_pos];
                                            if ti == 0 {
                                                let new_opened = !self.ui_state.lists[li].opened;
                                                self.ui_state.lists[li].opened = new_opened;
                                                if new_opened {
                                                    get_tasks(&self.api, &self.ui_state.filter, &mut self.ui_state.lists[li]).await?;
                                                }
                                            }
                                        }
                                        self.update_list_display().await?;
                                    }
                                }
                                StepResult::Cont
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                self.ui_state.list_pos = self.ui_state.list_pos.saturating_sub(1);
                                self.ui_state.list_state.select(Some(self.ui_state.list_pos));
                                StepResult::Cont
                            }
                            KeyCode::Down | KeyCode::Char('j')  => {
                                if self.ui_state.list_pos+1 < self.ui_state.list_items.len() {
                                    self.ui_state.list_pos += 1;
                                }
                                self.ui_state.list_state.select(Some(self.ui_state.list_pos));
                                StepResult::Cont
                            }
                            _ => StepResult::Cont,
                        }
                    }
                    _ => StepResult::Cont,
                }
            }
        };
        Ok(result)
    }

    pub async fn run(&mut self) -> Result<(), failure::Error> {
        loop {
            match self.step().await? {
                StepResult::End => break,
                StepResult::Cont => (), // continue
            }
        }
        Ok(())
    }
}

async fn get_tasks(api: &rememberthemilk::API, filter: &str, list_state: &mut ListDispState) -> Result<(), failure::Error> {
    let tasks = api.get_tasks_in_list(&list_state.list.id, filter).await?;
    list_state.tasks = Some(tasks);
    Ok(())
}

impl Drop for Tui {
    fn drop(&mut self) {
        disable_raw_mode().unwrap();
        self.terminal.show_cursor().unwrap();
    }
}

pub async fn tui() -> Result<(), failure::Error> {
    let mut tui = Tui::new().await?;

    tui.run().await?;

    Ok(())
}

