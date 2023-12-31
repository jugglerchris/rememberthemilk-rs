use chrono::{DateTime, Utc};
use rememberthemilk::{Perms, API, RTMTasks, RTMList, TaskSeries};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_stream::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    widgets::{Block, Borders, BorderType, Paragraph, Clear},
    Terminal, style::{Style, Color, Modifier}, text::{Line, Span}, layout::Rect
};
use tui_tree_widget::{
    Tree, TreeItem, TreeState
};
use crossterm::{terminal::{disable_raw_mode, enable_raw_mode}, event::{KeyCode, Event}};
use std::{io, borrow::Cow};
use std::collections::HashMap;

use crate::{get_rtm_api, get_default_filter, tail_end};

#[derive(Copy, Clone)]
enum DisplayMode {
    Tasks,
    Lists,
}

impl DisplayMode {
    fn title(&self) -> Cow<'static, str> {
        match self {
            DisplayMode::Tasks => "RTM Tasks".into(),
            DisplayMode::Lists => "RTM Lists".into(),
        }
    }
}

struct ListDispState {
    list: RTMList,
    tasks: Option<RTMTasks>,
}

struct UiState {
    display_mode: DisplayMode,
    filter: String,
    list_pos: usize,
    tree_items: Vec<TreeItem<'static, usize>>,
    tree_state: TreeState<usize>,
    list_paths: Vec<(usize, usize)>,
    tasks: RTMTasks,
    lists: Vec<ListDispState>,
    lists_loading: bool,
    show_task: bool,
    input_prompt: &'static str,
    input_value: String,
    show_input: bool,
    event_tx: Sender<TuiEvent>,
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

enum TuiEvent {
    Input(Result<crossterm::event::Event, std::io::Error>),
    StateChanged,
}

struct Tui {
    api: API,
    event_rx: Receiver<TuiEvent>,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: std::sync::Arc<std::sync::Mutex<UiState>>,
}
enum StepResult {
    Cont,
    End,
}
impl Tui {
    pub async fn new() -> Result<Tui, anyhow::Error> {
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut events = crossterm::event::EventStream::new();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(10);

        {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                event_tx.send(TuiEvent::StateChanged).await.map_err(|_|()).unwrap();
                while let Some(evt) = events.next().await {
                    event_tx.send(TuiEvent::Input(evt)).await.map_err(|_|()).unwrap();
                }
            });
        }

        let api = get_rtm_api(Perms::Read).await?;
        let tree_state: TreeState<usize> = Default::default();
        let filter = get_default_filter()?;
        let show_task = false;
        let display_mode = DisplayMode::Tasks;

        let ui_state = UiState {
            display_mode,
            filter,
            tree_state,
            list_pos: 0,
            tree_items: vec![],
            list_paths: vec![],
            tasks: Default::default(),
            lists: Default::default(),
            lists_loading: false,
            show_task,
            input_prompt: "",
            input_value: String::new(),
            show_input: false,
            event_tx,
        };

        let mut tui = Tui {
            api,
            event_rx,
            terminal,
            ui_state: std::sync::Arc::new(std::sync::Mutex::new(ui_state)),
        };
        tui.update_tasks().await?;

        Ok(tui)
    }
    async fn update_tasks(&mut self) -> Result<(), anyhow::Error> {
        let filter = self.ui_state.lock().unwrap().filter.clone();
        let tasks = self.api.get_tasks_filtered(&filter).await?;
        let list_pos = 0;

        let flat_tasks: Vec<_> = RtmTaskListIterator::new(&tasks).cloned().collect();

        // Map id to (is_root, TreeItem)
        let mut task_map = HashMap::new();
        let mut children_map = HashMap::new();
        // Map by id
        for (ti, ts) in RtmTaskListIterator::new(&tasks).enumerate() {
            let id = &ts.task[0].id;
            task_map.insert(id, (true, TreeItem::new_leaf(ti, ts.name.clone())));
            children_map.insert(id, Vec::new());
        }
        // Record children
        for (ti, ts) in RtmTaskListIterator::new(&tasks).enumerate() {
            let id = &ts.task[0].id;
            if let Some(parent_task_id) = &ts.parent_task_id {
                if !parent_task_id.is_empty() && task_map.contains_key(&parent_task_id) {
                    children_map
                        .get_mut(&parent_task_id)
                        .unwrap()
                        .push(ti);
                    // Mark as not root
                    task_map
                        .get_mut(id)
                        .unwrap()
                        .0 = false;
                }
            }
        }

        fn add_item(task_map: &mut HashMap<&String, (bool, TreeItem<'static, usize>)>, children_map: &mut HashMap<&String, Vec<usize>>, tasks: &Vec<TaskSeries>, list: &mut Vec<TreeItem<'static, usize>>, ti: usize, mut item: TreeItem<'static, usize>) {
            let id = &tasks[ti].task[0].id;
            let children = children_map.remove(id).unwrap();
            if !children.is_empty() {
                let mut child_items = Vec::new();
                for cti in children {
                    let cid = &tasks[cti].task[0].id;
                    let (_, citem) = task_map.remove(cid).unwrap();
                    add_item(task_map, children_map, tasks, &mut child_items, cti, citem);
                }
                for child in child_items {
                    item.add_child(child).unwrap();
                }
            }
            list.push(item);
        }
        let mut tree_items = Vec::new();
        for (ti, ts) in RtmTaskListIterator::new(&tasks).enumerate() {
            let id = &ts.task[0].id;
            let (is_root, _) = task_map.get(id).unwrap();
            if *is_root {
                let (_, item) = task_map.remove(id).unwrap();
                add_item(&mut task_map, &mut children_map, &flat_tasks, &mut tree_items, ti, item);
            }
        }
        if tree_items.is_empty() {
            tree_items.push(TreeItem::new_leaf(0, "[No tasks in current list]"));
        }
        {
            let mut ui_state = self.ui_state.lock().unwrap();
            ui_state.tree_state.select_first(&tree_items);
            ui_state.tasks = tasks;
            ui_state.tree_items = tree_items;
            ui_state.list_pos = list_pos;
            ui_state.display_mode = DisplayMode::Tasks;
        }
        Ok(())
    }

    async fn update_list_display(&mut self) -> Result<(), anyhow::Error> {
        let mut tree_items = vec![];
        let mut list_paths = vec![];
        let mut ui_state = self.ui_state.lock().unwrap();
        for (i, list) in ui_state.lists.iter().enumerate() {
            if let Some(tasks) = list.tasks.as_ref() {
                let len: usize =
                   tasks
                    .list
                    .iter()
                    .map(|l| l.taskseries.as_ref().map(|ts| ts.len()).unwrap_or(0))
                    .sum();
                if len > 0 {
                    let mut item =
                        TreeItem::new_leaf(
                                i,
                                format!("{} [{}]", &list.list.name, len)
                            ).style(Style::default().fg(Color::LightYellow));
                    if let Some(tasks) = list.tasks.as_ref() {
                        for (ti, task) in RtmTaskListIterator::new(tasks).enumerate()
                        {
                            item.add_child(TreeItem::new_leaf(ti, format!("  {}", task.name))).unwrap();
                        }
                    }
                    tree_items.push(item);
                } else {
                    tree_items.push(
                        TreeItem::new_leaf(
                            i,
                            format!("{}", &list.list.name)
                            ).style(Style::default().fg(Color::DarkGray)));
                }
            } else {
                tree_items.push(TreeItem::new_leaf(i, list.list.name.clone())
                            .style(Style::default().fg(Color::White)));
            }
            list_paths.push((i, 0));
        }
        if tree_items.is_empty() {
            if ui_state.lists_loading {
                tree_items.push(TreeItem::new_leaf(0, "Loading..."));
            } else {
                tree_items.push(TreeItem::new_leaf(0, "[No lists]"));
            }
        }
        ui_state.tree_items = tree_items;
        ui_state.list_paths = list_paths;
        Ok(())
    }

    async fn fetch_lists(api: API, ui_state: std::sync::Arc<std::sync::Mutex<UiState>>) {
        let lists = api.get_lists().await.unwrap();
        let tx = ui_state.lock().unwrap().event_tx.clone();
        {
            let mut ui_state = ui_state.lock().unwrap();
            ui_state.lists = lists.into_iter().map(|l| ListDispState {
                list: l,
                tasks: None,
            }).collect();
            ui_state.lists_loading = false;
        }
        tx.send(TuiEvent::StateChanged).await.map_err(|_|()).unwrap();

        // Now fetch each list
        let (filter, ids) = {
            let ui_state = ui_state.lock().unwrap();
            let mut ids = Vec::new();
            for (i, list_state) in ui_state.lists.iter().enumerate() {
                ids.push((i, list_state.list.id.clone()));
            }

            (ui_state.filter.clone(), ids)
        };
        for (idx, list_id) in ids {
            let tasks = get_tasks(&api, &filter, &list_id).await.unwrap();
            {
                let mut ui_state = ui_state.lock().unwrap();
                if ui_state.lists.len() > idx && ui_state.lists[idx].list.id == list_id {
                    ui_state.lists[idx].tasks = Some(tasks);

                } else {
                    // We're out of step - return
                    return;
                }
            }
            tx.send(TuiEvent::StateChanged).await.map_err(|_|()).unwrap();
        }
        ui_state.lock().unwrap().lists_loading = false;
    }

    async fn update_lists(&mut self) -> Result<(), anyhow::Error> {
        {
            let mut ui_state = self.ui_state.lock().unwrap();
            let ui_state = &mut *ui_state;
            ui_state.display_mode = DisplayMode::Lists;
            ui_state.lists_loading = true;
            ui_state.tree_state.select_first(&ui_state.tree_items[..]);

            ui_state.list_pos = 0;
            ui_state.show_task = false;

        }
        tokio::spawn(Tui::fetch_lists(self.api.clone(), std::sync::Arc::clone(&self.ui_state)));
        self.update_list_display().await
    }

    async fn draw(&mut self) -> Result<(), anyhow::Error> {
        let mut ui_state = self.ui_state.lock().unwrap();
        self.terminal.draw(move |f| {
            let size = f.size();
            let block = Block::default()
                .title(ui_state.display_mode.title().into_owned())
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black));
            let tree = Tree::new(ui_state.tree_items.clone())
                            .unwrap()
                            .block(block)
                            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                            .highlight_symbol("*");
            let mut list_size = size;
            if ui_state.show_task {
                list_size.height = list_size.height / 2;
            }
            f.render_stateful_widget(tree, list_size, &mut ui_state.tree_state);

            if ui_state.show_task {
                let block = Block::default()
                    .title("Task")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::White))
                    .border_type(BorderType::Rounded)
                    .style(Style::default().bg(Color::Black));
                let tree_pos = ui_state.tree_state.selected();
                let series = match ui_state.display_mode {
                    DisplayMode::Tasks => {
                        Some(RtmTaskListIterator::new(&ui_state.tasks).nth(*tree_pos.last().unwrap()).unwrap())
                    }
                    DisplayMode::Lists => {
                        if tree_pos.len() == 2 {
                            Some(RtmTaskListIterator::new(
                                    ui_state.lists[tree_pos[0]]
                                    .tasks
                                    .as_ref()
                                    .unwrap())
                                .nth(tree_pos[1])
                                .unwrap())
                        } else {
                            None
                        }
                    }
                };

                if let Some(series) = series {
                    let mut text = vec![
                        Line::from(vec![
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
                        text.push( Line::from(spans));
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
                        text.push( Line::from(spans));
                    }
                    for task in &series.task {
                        fn add_date_field(text: &mut Vec<Line>, heading: &'static str,
                                          value: &Option<DateTime<Utc>>,
                                          color: Color) {
                            if let Some(date) = value {
                                let style = Style::default()
                                    .fg(color)
                                    .add_modifier(Modifier::BOLD);
                                let mut spans = vec![
                                    Span::raw(heading)];
                                spans.push(
                                    Span::styled(format!("{}", date.format("%c")), style));
                                text.push(Line::from(spans));
                            }
                        }
                        add_date_field(&mut text, "Due: ", &task.due, Color::Yellow);
                        add_date_field(&mut text, "Completed: ", &task.completed, Color::Magenta);
                        add_date_field(&mut text, "Deleted: ", &task.deleted, Color::Red);
                    }
                    fn add_string_field(text: &mut Vec<Line>, heading: &'static str,
                                        value: &str,
                                        color: Color) {
                        if !value.is_empty() {
                            let style = Style::default()
                                .fg(color)
                                .add_modifier(Modifier::BOLD);
                            let mut spans = vec![
                                Span::raw(heading)];
                            spans.push(
                                Span::styled(value.to_owned(), style));
                            text.push(Line::from(spans));
                        }
                    }
                    add_string_field(&mut text, "URL: ", &series.url, Color::Yellow);
                    add_string_field(&mut text, "Source: ", &series.source, Color::Yellow);
                    if !series.notes.is_empty() {
                        text.push(Line::from(vec![Span::raw("Notes:")]));
                        for note in &series.notes {
                            add_string_field(&mut text, "  ", &note.text, Color::White);
                        }
                    }

                    let par = Paragraph::new(text)
                        .block(block);
                    let area = Rect::new(
                        0, list_size.height,
                        size.width, size.height - list_size.height);
                    f.render_widget(par, area);
                }
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
                    Paragraph::new(vec![Line::from(text)])
                        .block(block), area);
            }
        })?;
        Ok(())
    }

    async fn input(&mut self, prompt: &'static str, default: &str) -> Result<String, anyhow::Error> {
        {
            let mut ui_state = self.ui_state.lock().unwrap();
            ui_state.input_value = default.into();
            ui_state.input_prompt = prompt;
            ui_state.show_input = true;
        }
        loop {
            self.draw().await?;
            match self.event_rx.recv().await {
                None => { return Ok("".into()); }
                Some(TuiEvent::Input(ev)) => match ev {
                    Err(e) => { return Err(e.into()); }
                    Ok(ev) => match ev {
                        Event::Key(key) => {
                            use crossterm::event::KeyModifiers;
                            match (key.code, key.modifiers) {
                                (KeyCode::Char(c), KeyModifiers::NONE) => {
                                    self.ui_state.lock().unwrap().input_value.push(c);
                                }
                                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                    self.ui_state.lock().unwrap().input_value.clear();
                                }
                                (KeyCode::Enter, KeyModifiers::NONE) => {
                                    break;
                                }
                                (KeyCode::Backspace, KeyModifiers::NONE) => {
                                    let _ = self.ui_state.lock().unwrap().input_value.pop();
                                }
                                (KeyCode::Esc, KeyModifiers::NONE) => {
                                    self.ui_state.lock().unwrap().show_input = false;
                                    return Ok(String::new());
                                }
                                _ => (),
                            }
                        }
                        _ => ()
                    }
                }
                Some(TuiEvent::StateChanged) => (),
            }
        }
        let mut ui_state = self.ui_state.lock().unwrap();
        ui_state.show_input = false;

        let mut result = String::new();
        std::mem::swap(&mut result, &mut ui_state.input_value);
        Ok(result)
    }

    pub async fn step(&mut self) -> Result<StepResult, anyhow::Error> {
        self.draw().await?;

        let result = match self.event_rx.recv().await {
            None => { return Ok(StepResult::End); }
            Some(TuiEvent::Input(ev)) => match ev {
                Err(e) => { return Err(e.into()); }
                Ok(ev) => match ev {
                    Event::Key(key) => {
                        match key.code {
                            KeyCode::Char('q') => {
                                StepResult::End
                            }
                            KeyCode::Char('g') => {
                                let cur_filt = self.ui_state.lock().unwrap().filter.clone();
                                let filter = self.input("Enter RTM filter:", &cur_filt).await?;
                                if !filter.is_empty() {
                                    self.ui_state.lock().unwrap().filter = filter;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            KeyCode::Char('A') => {
                                let task_desc = self.input("Enter new task:", "").await?;
                                if !task_desc.is_empty() {
                                    let timeline = self.api.get_timeline().await?;
                                    let _added = self.api.add_task(
                                        &timeline,
                                        &task_desc,
                                        None, None, None,
                                        true).await?;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            KeyCode::Char('L') => {
                                self.update_lists().await?;
                                StepResult::Cont
                            }
                            KeyCode::Enter => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                match ui_state.display_mode {
                                    DisplayMode::Tasks => {
                                        ui_state.show_task = !ui_state.show_task;
                                    }
                                    DisplayMode::Lists => {
                                        ui_state.show_task = !ui_state.show_task;
                                    }
                                }
                                StepResult::Cont
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                let ui_state = &mut *ui_state;
                                ui_state.list_pos = ui_state.list_pos.saturating_sub(1);
                                ui_state.tree_state.key_up(&ui_state.tree_items[..]);
                                StepResult::Cont
                            }
                            KeyCode::Down | KeyCode::Char('j')  => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                let ui_state = &mut *ui_state;
                                if ui_state.list_pos+1 < ui_state.tree_items.len() {
                                    ui_state.list_pos += 1;
                                }
                                ui_state.tree_state.key_down(&ui_state.tree_items[..]);
                                StepResult::Cont
                            }
                            KeyCode::Char(' ') => {
                                let mut ui_state = self.ui_state.lock().unwrap();
                                let ui_state = &mut *ui_state;
                                ui_state.tree_state.toggle_selected();
                                StepResult::Cont
                            }
                            _ => StepResult::Cont,
                        }
                    }
                    _ => StepResult::Cont,
                }
            }
            Some(TuiEvent::StateChanged) => {
                let display_mode = self.ui_state.lock().unwrap().display_mode;
                match display_mode {
                    DisplayMode::Tasks => {}
                    DisplayMode::Lists => {
                        self.update_list_display().await?;
                    }
                }
                StepResult::Cont
            }
        };
        Ok(result)
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        loop {
            match self.step().await? {
                StepResult::End => break,
                StepResult::Cont => (), // continue
            }
        }
        Ok(())
    }
}

async fn get_tasks(api: &rememberthemilk::API, filter: &str, id: &str) -> Result<RTMTasks, anyhow::Error> {
    let tasks = api.get_tasks_in_list(id, filter).await?;
    Ok(tasks)
}

impl Drop for Tui {
    fn drop(&mut self) {
        disable_raw_mode().unwrap();
        self.terminal.show_cursor().unwrap();
    }
}

pub async fn tui() -> Result<(), anyhow::Error> {
    let mut tui = Tui::new().await?;

    tui.run().await?;

    Ok(())
}

