use chrono::{DateTime, Utc};
use rememberthemilk::{Perms, API, RTMTasks};
use tokio_stream::StreamExt;
use tui::{
    backend::CrosstermBackend,
    widgets::{List, Block, Borders, BorderType, ListItem, ListState, Paragraph, Clear},
    Terminal, style::{Style, Color, Modifier}, text::{Spans, Span}, layout::Rect
};
use crossterm::{terminal::{disable_raw_mode, enable_raw_mode}, event::{KeyCode, Event, EventStream}};
use std::io;

use crate::{get_rtm_api, get_default_filter, tail_end};

struct Tui {
    api: API,
    filter: String,
    list_state: ListState,
    list_pos: usize,
    list_items: Vec<ListItem<'static>>,
    list_paths: Vec<(usize, usize)>,
    tasks: RTMTasks,
    events: EventStream,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    show_task: bool,
    input_prompt: &'static str,
    input_value: String,
    show_input: bool,
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

        let mut tui = Tui {
            api,
            filter,
            list_state,
            list_pos: 0,
            list_items: vec![],
            list_paths: vec![],
            tasks: Default::default(),
            events,
            terminal,
            show_task,
            input_prompt: "",
            input_value: String::new(),
            show_input: false,
        };
        tui.update_tasks().await?;

        Ok(tui)
    }
    async fn update_tasks(&mut self) -> Result<(), failure::Error> {
        let tasks = self.api.get_tasks_filtered(&self.filter).await?;
        let list_pos = 0;
        self.list_state.select(Some(list_pos));

        let mut list_items = vec![];
        let mut list_paths = vec![];
        for (li, list) in tasks.list.iter().enumerate() {
            if let Some(v) = &list.taskseries {
                for (ti, ts) in v.iter().enumerate() {
                    list_paths.push((li, ti));
                    list_items.push(ListItem::new(ts.name.clone()));
                }
            }
        }
        if list_items.is_empty() {
            list_items.push(ListItem::new("[No tasks in current list]"));
        }
        self.tasks = tasks;
        self.list_items = list_items;
        self.list_paths = list_paths;
        self.list_pos = list_pos;
        Ok(())
    }

    async fn draw(&mut self) -> Result<(), failure::Error> {
        let list_state = &mut self.list_state;
        let list_items = &self.list_items;
        let list_paths = &self.list_paths;
        let show_task = self.show_task;
        let show_input = self.show_input;
        let list_pos = self.list_pos;
        let tasks = &self.tasks;
        let input_prompt = self.input_prompt;
        let input_value = &self.input_value;
        self.terminal.draw(move |f| {
            let size = f.size();
            let block = Block::default()
                .title("RTM list")
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black));
            let list = List::new(&list_items[..])
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("*");
            let mut list_size = size;
            if show_task {
                list_size.height = list_size.height / 2;
            }
            f.render_stateful_widget(list, list_size, list_state);

            if show_task && !list_paths.is_empty() {
                let block = Block::default()
                    .title("Task")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::White))
                    .border_type(BorderType::Rounded)
                    .style(Style::default().bg(Color::Black));
                let (li, ti) = list_paths[list_pos];
                let list = &tasks.list[li];
                let series = &list.taskseries.as_ref().unwrap()[ti];
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
            if show_input {
                let block = Block::default()
                    .title(input_prompt)
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::White))
                    .style(Style::default().bg(Color::Black));
                let area = Rect::new(0, size.height-2, size.width, 2);
                f.render_widget(Clear, area);

                let visible_value = tail_end(input_value, size.width as usize -1);
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
        self.input_value = self.filter.clone();
        self.input_prompt = prompt;
        self.show_input = true;
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
                                    self.input_value.push(c);
                                }
                                KeyCode::Enter => {
                                    break;
                                }
                                KeyCode::Backspace => {
                                    let _ = self.input_value.pop();
                                }
                                KeyCode::Esc => {
                                    self.show_input = false;
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
        self.show_input = false;

        let mut result = String::new();
        std::mem::swap(&mut result, &mut self.input_value);
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
                                    self.filter = filter;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            KeyCode::Enter => {
                                self.show_task = !self.show_task;
                                StepResult::Cont
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                self.list_pos = self.list_pos.saturating_sub(1);
                                self.list_state.select(Some(self.list_pos));
                                StepResult::Cont
                            }
                            KeyCode::Down | KeyCode::Char('j')  => {
                                if self.list_pos+1 < self.list_items.len() {
                                    self.list_pos += 1;
                                }
                                self.list_state.select(Some(self.list_pos));
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

