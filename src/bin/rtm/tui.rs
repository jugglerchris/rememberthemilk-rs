use chrono::{DateTime, Utc};
use crossterm::{
    event::{Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use log::{info, trace};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Terminal,
};
use rememberthemilk::{
    cache::TaskCache, Perms, RTMList, RTMLists, RTMTasks, RTMTimeline, Task, TaskSeries,
};
use std::process::ExitCode;
use std::{borrow::Cow, io};
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_stream::StreamExt;
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::{get_default_filter, get_rtm_api, get_rtm_cache};

static HELP_TEXT: &str = r#"Key bindings:

A       New task
C       Mark current task complete
g       Change filter
L       View lists
q       Quit
Up/k    Move up one
Down/j  Move down one
Space   Toggle selection
?/h     Show this help
enter   Toggle task details
^L      Refresh screen
^R      Initiate sync
"#;

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

struct TaskInfo {
    ts: TaskSeries,
    list_id: String,
}

struct UiState {
    display_mode: DisplayMode,
    filter: String,
    list_pos: usize,
    tree_items: Vec<TreeItem<'static, usize>>,
    tree_state: TreeState<usize>,
    list_paths: Vec<(usize, usize)>,
    flat_tasks: Vec<TaskInfo>,
    lists: Vec<ListDispState>,
    lists_loading: bool,
    show_task: bool,
    input_prompt: &'static str,
    input_value: String,
    show_input: bool,
    show_help: bool,
    refresh: bool,
    // Spinner with current state.
    spinner: Option<(String, usize, &'static [&'static str])>,
    tick_running: bool,
    tick_tx: Sender<()>,
    event_tx: Sender<TuiEvent>,
}

impl UiState {
    // Returns true if the tick should be rescheduled
    fn tick(&mut self) {
        if let Some((_s, step, chars)) = self.spinner.as_mut() {
            *step = (*step + 1) % chars.len();
        } else {
            self.stop_ticking();
        }
    }

    // Start a tick
    async fn start_progress<F: Future + Send + 'static>(
        ui: &std::sync::Arc<tokio::sync::Mutex<Self>>,
        message: &str,
        spinner: &'static [&'static str],
        task: F,
    ) -> anyhow::Result<()> {
        let mut this = ui.lock().await;
        this.tick_running = true;
        this.spinner = Some((message.into(), 0, spinner));
        {
            let task_this = Arc::clone(ui);
            tokio::spawn(async move {
                task.await;
                let mut locked = task_this.lock().await;
                locked.spinner = None;
                locked.tick_running = false;
            });
        }
        this.tick_tx.send(()).await?;
        Ok(())
    }

    // Stop ticking
    fn stop_ticking(&mut self) {
        self.tick_running = false;
    }
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
    type Item = (&'t RTMLists, &'t TaskSeries);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let list_idx = match self.next_list_idx {
                Some(idx) => idx,
                None => {
                    return None;
                }
            };
            if list_idx >= self.tasks.list.len() {
                // We're done.
                self.next_list_idx = None;
                return None;
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
                return Some((list, &vseries[idx]));
            } else {
                // No taskseries
                self.next_list_idx = Some(list_idx + 1);
                self.next_task_idx = 0;
                continue;
            }
        }
    }
}

enum TuiEvent {
    Input(Result<crossterm::event::Event, std::io::Error>),
    StateChanged,
    Tick,
    SyncFinished,
    ListSyncFinished,
}

struct Tui {
    api_cache: TaskCache,
    current_timeline: Option<RTMTimeline>,
    transactions: Vec<String>,
    event_rx: Receiver<TuiEvent>,
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    ui_state: std::sync::Arc<tokio::sync::Mutex<UiState>>,
}
enum StepResult {
    Cont,
    End,
}

fn tail_end(input: &str, width: usize) -> String {
    let tot_width = unicode_width::UnicodeWidthStr::width(input);
    if tot_width <= width {
        // It fits, no problem.
        return input.into();
    }
    // Otherwise, trim off the start, making space for a ...
    let mut result = "…".to_string();
    let elipsis_width = unicode_width::UnicodeWidthStr::width(result.as_str());
    let space_needed = tot_width - (width - elipsis_width);

    let mut removed_space = 0;
    let mut ci = input.char_indices();

    for (_, c) in &mut ci {
        if let Some(w) = unicode_width::UnicodeWidthChar::width(c) {
            removed_space += w;
            if removed_space >= space_needed {
                break;
            }
        }
    }
    let (start, _) = ci.next().unwrap();
    result.push_str(&input[start..]);
    result
}

impl Tui {
    pub async fn new() -> Result<Tui, anyhow::Error> {
        info!("Setting up terminal...");
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut events = crossterm::event::EventStream::new();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(10);

        {
            let event_tx = event_tx.clone();
            info!("Spawning event task...");
            tokio::spawn(async move {
                event_tx
                    .send(TuiEvent::StateChanged)
                    .await
                    .map_err(|_| ())
                    .unwrap();
                while let Some(evt) = events.next().await {
                    event_tx
                        .send(TuiEvent::Input(evt))
                        .await
                        .map_err(|_| ())
                        .unwrap();
                }
            });
        }

        let (tick_tx, mut tick_rx) = tokio::sync::mpsc::channel(1);
        info!("Getting API instance...");
        let api = get_rtm_api(Perms::Delete).await?;
        let api_cache = get_rtm_cache(api).await?;
        let tree_state: TreeState<usize> = Default::default();
        let filter = get_default_filter()?;
        let show_task = false;
        let show_help = false;
        let display_mode = DisplayMode::Tasks;

        let ui_state = UiState {
            display_mode,
            filter,
            tree_state,
            list_pos: 0,
            tree_items: vec![],
            list_paths: vec![],
            flat_tasks: Default::default(),
            lists: Default::default(),
            lists_loading: false,
            show_task,
            show_help,
            input_prompt: "",
            input_value: String::new(),
            show_input: false,
            refresh: false,
            event_tx: event_tx.clone(),
            spinner: None,
            tick_running: false,
            tick_tx,
        };

        let mut tui = Tui {
            api_cache,
            event_rx,
            terminal,
            ui_state: std::sync::Arc::new(tokio::sync::Mutex::new(ui_state)),
            current_timeline: None,
            transactions: Vec::new(),
        };

        {
            let ui_state = Arc::clone(&tui.ui_state);
            tokio::spawn(async move {
                loop {
                    let _ = tick_rx.recv().await;
                    let running = ui_state.lock().await.tick_running;
                    if running {
                        // Start ticking.
                        let mut interval = tokio::time::interval(Duration::from_millis(250));
                        loop {
                            let _ = interval.tick().await;
                            let ui_state = ui_state.lock().await;
                            if ui_state.tick_running {
                                event_tx.send(TuiEvent::Tick).await.unwrap();
                            } else {
                                break;
                            }
                        }
                    }
                }
            });
        }

        info!("Updating tasks...");
        tui.update_tasks().await?;
        info!("TUI ready.");

        Ok(tui)
    }
    async fn update_tasks(&mut self) -> Result<(), anyhow::Error> {
        trace!("Getting filter...");
        let filter = self.ui_state.lock().await.filter.clone();
        trace!("Requesting tasks...");
        let tasks = self.api_cache.get_tasks_filtered(&filter).await?;
        trace!("Got tasks.");
        let tasks = self.add_missing_children(tasks).await?;
        let list_pos = 0;

        let flat_tasks: Vec<TaskInfo> = {
            let mut ft: Vec<_> = RtmTaskListIterator::new(&tasks)
                .map(|(l, t)| TaskInfo {
                    ts: t.clone(),
                    list_id: l.id.clone(),
                })
                .collect();
            ft.sort_by(|a, b| (&a.ts.name, &a.ts.id).cmp(&(&b.ts.name, &b.ts.id)));
            ft
        };

        // Map id to (is_root, TreeItem)
        let mut task_map = HashMap::new();
        let mut children_map = HashMap::new();
        // Map by id
        for (ti, tinfo) in flat_tasks.iter().enumerate() {
            let ts = &tinfo.ts;
            let id = &ts.task[0].id;
            task_map.insert(id, (true, TreeItem::new_leaf(ti, ts.name.clone())));
            children_map.insert(id, Vec::new());
        }
        // Record children
        for (ti, tinfo) in flat_tasks.iter().enumerate() {
            let ts = &tinfo.ts;
            let id = &ts.task[0].id;
            if let Some(parent_task_id) = &ts.parent_task_id {
                if !parent_task_id.is_empty() && task_map.contains_key(&parent_task_id) {
                    children_map.get_mut(&parent_task_id).unwrap().push(ti);
                    // Mark as not root
                    task_map.get_mut(id).unwrap().0 = false;
                }
            }
        }

        fn add_item(
            task_map: &mut HashMap<&String, (bool, TreeItem<'static, usize>)>,
            children_map: &mut HashMap<&String, Vec<usize>>,
            tasks: &Vec<TaskInfo>,
            list: &mut Vec<TreeItem<'static, usize>>,
            ti: usize,
            mut item: TreeItem<'static, usize>,
        ) {
            let id = &tasks[ti].ts.task[0].id;
            let children = children_map.remove(id).unwrap();
            if !children.is_empty() {
                let mut child_items = Vec::new();
                for cti in children {
                    let cid = &tasks[cti].ts.task[0].id;
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
        for (ti, tinfo) in flat_tasks.iter().enumerate() {
            let ts = &tinfo.ts;
            let id = &ts.task[0].id;
            if let Some((true, _)) = task_map.get(id) {
                let (_, item) = task_map.remove(id).unwrap();
                add_item(
                    &mut task_map,
                    &mut children_map,
                    &flat_tasks,
                    &mut tree_items,
                    ti,
                    item,
                );
            }
        }
        if tree_items.is_empty() {
            tree_items.push(TreeItem::new_leaf(0, "[No tasks in current list]"));
        }
        {
            let mut ui_state = self.ui_state.lock().await;
            ui_state.tree_state.select_first();
            ui_state.flat_tasks = flat_tasks;
            ui_state.tree_items = tree_items;
            ui_state.list_pos = list_pos;
            ui_state.display_mode = DisplayMode::Tasks;
        }
        Ok(())
    }

    async fn update_list_display(&mut self) -> Result<(), anyhow::Error> {
        let mut tree_items = vec![];
        let mut list_paths = vec![];
        let mut ui_state = self.ui_state.lock().await;
        for (i, list) in ui_state.lists.iter().enumerate() {
            if let Some(tasks) = list.tasks.as_ref() {
                let len: usize = tasks
                    .list
                    .iter()
                    .map(|l| l.taskseries.as_ref().map(|ts| ts.len()).unwrap_or(0))
                    .sum();
                if len > 0 {
                    let mut item = TreeItem::new_leaf(
                        i,
                        Text::styled(
                            format!("{} [{}]", &list.list.name, len),
                            Style::default().fg(Color::LightYellow),
                        ),
                    );
                    if let Some(tasks) = list.tasks.as_ref() {
                        for (ti, (_l, task)) in RtmTaskListIterator::new(tasks).enumerate() {
                            item.add_child(TreeItem::new_leaf(ti, format!("  {}", task.name)))
                                .unwrap();
                        }
                    }
                    tree_items.push(item);
                } else {
                    tree_items.push(TreeItem::new_leaf(
                        i,
                        Text::styled(
                            list.list.name.to_string(),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ));
                }
            } else {
                tree_items.push(TreeItem::new_leaf(
                    i,
                    Text::styled(list.list.name.clone(), Style::default().fg(Color::White)),
                ));
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

    async fn fetch_lists(
        api_cache: TaskCache,
        ui_state: std::sync::Arc<tokio::sync::Mutex<UiState>>,
    ) {
        let lists = api_cache.get_lists().await.unwrap();
        let tx = ui_state.lock().await.event_tx.clone();
        {
            let mut ui_state = ui_state.lock().await;
            ui_state.lists = lists
                .into_iter()
                .map(|l| ListDispState {
                    list: l,
                    tasks: None,
                })
                .collect();
            ui_state.lists_loading = false;
        }
        tx.send(TuiEvent::StateChanged)
            .await
            .map_err(|_| ())
            .unwrap();

        // Now fetch each list
        let (filter, ids) = {
            let ui_state = ui_state.lock().await;
            let mut ids = Vec::new();
            for (i, list_state) in ui_state.lists.iter().enumerate() {
                ids.push((i, list_state.list.id.clone()));
            }

            (ui_state.filter.clone(), ids)
        };
        for (idx, list_id) in ids {
            let tasks = get_tasks(&api_cache, &filter, &list_id).await.unwrap();
            {
                let mut ui_state = ui_state.lock().await;
                if ui_state.lists.len() > idx && ui_state.lists[idx].list.id == list_id {
                    ui_state.lists[idx].tasks = Some(tasks);
                } else {
                    // We're out of step - return
                    return;
                }
            }
            tx.send(TuiEvent::StateChanged)
                .await
                .map_err(|_| ())
                .unwrap();
        }
        ui_state.lock().await.lists_loading = false;
    }

    async fn update_lists(&mut self) -> Result<(), anyhow::Error> {
        let event_tx = {
            let mut ui_state = self.ui_state.lock().await;
            let ui_state = &mut *ui_state;
            ui_state.display_mode = DisplayMode::Lists;
            ui_state.lists_loading = true;
            ui_state.tree_state.select_first();

            ui_state.list_pos = 0;
            ui_state.show_task = false;
            ui_state.event_tx.clone()
        };
        let api_cache = self.api_cache.clone();
        let ui_state_ptr = std::sync::Arc::clone(&self.ui_state);
        UiState::start_progress(
            &self.ui_state,
            "fetching lists...",
            &[".", "o", "O"],
            async move {
                Tui::fetch_lists(api_cache, ui_state_ptr).await;
                event_tx.send(TuiEvent::ListSyncFinished).await.unwrap();
            },
        )
        .await?;
        self.update_list_display().await
    }

    async fn draw(&mut self) -> Result<(), anyhow::Error> {
        let mut ui_state = self.ui_state.lock().await;
        if ui_state.refresh {
            self.terminal.draw(move |f| {
                f.render_widget(Clear, f.area());
            })?;
            ui_state.refresh = false;
        }
        self.terminal.draw(move |f| {
            let size = f.area();
            let block = Block::default()
                .title(ui_state.display_mode.title().into_owned())
                .borders(Borders::TOP | Borders::BOTTOM)
                .border_style(Style::default().fg(Color::White))
                .border_type(BorderType::Rounded)
                .style(Style::default().bg(Color::Black));
            let block = if let Some((msg, step, chars)) = &ui_state.spinner {
                block.title_bottom(format!("{msg} {}", chars[*step]))
            } else {
                block
            };
            let tree_items = ui_state.tree_items.clone();
            let tree = Tree::new(&tree_items)
                .unwrap()
                .block(block)
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("*");
            let mut list_size = size;
            if ui_state.show_task {
                list_size.height /= 2;
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
                    DisplayMode::Tasks => tree_pos.last().map(|pos| &ui_state.flat_tasks[*pos].ts),
                    DisplayMode::Lists => {
                        if tree_pos.len() == 2 {
                            Some(
                                RtmTaskListIterator::new(
                                    ui_state.lists[tree_pos[0]].tasks.as_ref().unwrap(),
                                )
                                .nth(tree_pos[1])
                                .unwrap()
                                .1,
                            )
                        } else {
                            None
                        }
                    }
                };

                if let Some(series) = series {
                    let mut text = vec![Line::from(vec![Span::raw(series.name.clone())])];
                    if !series.tags.is_empty() {
                        let mut spans = vec![Span::raw("Tags: ")];
                        let tag_style = Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD);
                        for tag in &series.tags {
                            spans.push(Span::styled(tag.clone(), tag_style));
                            spans.push(" ".into());
                        }
                        text.push(Line::from(spans));
                    }
                    if let Some(repeat) = &series.repeat {
                        let style = Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD);
                        let mut spans = vec![Span::raw("Repeat: ")];
                        if repeat.every {
                            spans.push(Span::raw("every "));
                        } else {
                            spans.push(Span::raw("after "));
                        }
                        spans.push(Span::styled(repeat.rule.clone(), style));
                        text.push(Line::from(spans));
                    }
                    for task in &series.task {
                        fn add_date_field(
                            text: &mut Vec<Line>,
                            heading: &'static str,
                            value: &Option<DateTime<Utc>>,
                            color: Color,
                        ) {
                            if let Some(date) = value {
                                let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                                let mut spans = vec![Span::raw(heading)];
                                spans.push(Span::styled(format!("{}", date.format("%c")), style));
                                text.push(Line::from(spans));
                            }
                        }
                        add_date_field(&mut text, "Due: ", &task.due, Color::Yellow);
                        add_date_field(&mut text, "Completed: ", &task.completed, Color::Magenta);
                        add_date_field(&mut text, "Deleted: ", &task.deleted, Color::Red);
                    }
                    fn add_string_field(
                        text: &mut Vec<Line>,
                        heading: &'static str,
                        value: &str,
                        color: Color,
                    ) {
                        if !value.is_empty() {
                            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                            let mut spans = vec![Span::raw(heading)];
                            spans.push(Span::styled(value.to_owned(), style));
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

                    let par = Paragraph::new(text).block(block);
                    let area = Rect::new(
                        0,
                        list_size.height,
                        size.width,
                        size.height - list_size.height,
                    );
                    f.render_widget(par, area);
                }
            }
            if ui_state.show_input {
                let block = Block::default()
                    .title(ui_state.input_prompt)
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::White))
                    .style(Style::default().bg(Color::Black));
                let area = Rect::new(0, size.height - 2, size.width, 2);
                f.render_widget(Clear, area);

                let visible_value = tail_end(&ui_state.input_value, size.width as usize - 1);
                let text = vec![Span::raw(visible_value), Span::raw("_")];
                f.render_widget(Paragraph::new(vec![Line::from(text)]).block(block), area);
            }
            if ui_state.show_help {
                let block = Block::default()
                    .title("Help")
                    .borders(Borders::all())
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().fg(Color::Green));
                let help_text = HELP_TEXT;
                let (max_w, max_h) = help_text
                    .lines()
                    .fold((0, 0), |(mw, mh), line| (mw.max(line.len()), mh + 1));
                let area = Rect::new(1, 1, max_w as u16 + 2, max_h as u16 + 2);
                f.render_widget(Clear, area);
                f.render_widget(Paragraph::new(Text::raw(help_text)).block(block), area);
            }
        })?;
        Ok(())
    }

    async fn input(
        &mut self,
        prompt: &'static str,
        default: &str,
    ) -> Result<String, anyhow::Error> {
        {
            let mut ui_state = self.ui_state.lock().await;
            ui_state.input_value = default.into();
            ui_state.input_prompt = prompt;
            ui_state.show_input = true;
        }
        loop {
            self.draw().await?;
            match self.event_rx.recv().await {
                None => {
                    return Ok("".into());
                }
                Some(TuiEvent::Input(ev)) => match ev {
                    Err(e) => {
                        return Err(e.into());
                    }
                    Ok(ev) => {
                        if let Event::Key(key) = ev {
                            use crossterm::event::KeyModifiers;
                            match (key.code, key.modifiers) {
                                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                    self.ui_state.lock().await.input_value.push(c);
                                }
                                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                    self.ui_state.lock().await.input_value.clear();
                                }
                                (KeyCode::Enter, KeyModifiers::NONE) => {
                                    break;
                                }
                                (KeyCode::Backspace, KeyModifiers::NONE) => {
                                    let _ = self.ui_state.lock().await.input_value.pop();
                                }
                                (KeyCode::Esc, KeyModifiers::NONE) => {
                                    self.ui_state.lock().await.show_input = false;
                                    return Ok(String::new());
                                }
                                _ => (),
                            }
                        }
                    }
                },
                Some(TuiEvent::StateChanged) => (),
                Some(TuiEvent::Tick) => {
                    self.ui_state.lock().await.tick();
                }
                Some(TuiEvent::SyncFinished) => {
                    self.update_tasks().await.unwrap();
                }
                Some(TuiEvent::ListSyncFinished) => {}
            }
        }
        let mut ui_state = self.ui_state.lock().await;
        ui_state.show_input = false;

        let mut result = String::new();
        std::mem::swap(&mut result, &mut ui_state.input_value);
        Ok(result)
    }

    pub async fn step(&mut self) -> Result<StepResult, anyhow::Error> {
        self.draw().await?;

        let result = match self.event_rx.recv().await {
            None => {
                return Ok(StepResult::End);
            }
            Some(TuiEvent::Input(ev)) => match ev {
                Err(e) => {
                    return Err(e.into());
                }
                Ok(ev) => {
                    use crossterm::event::KeyModifiers;
                    match ev {
                        Event::Key(key) => match (key.code, key.modifiers) {
                            (KeyCode::Char('q'), KeyModifiers::NONE) => StepResult::End,
                            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                                let cur_filt = self.ui_state.lock().await.filter.clone();
                                let filter = self.input("Enter RTM filter:", &cur_filt).await?;
                                if !filter.is_empty() {
                                    self.ui_state.lock().await.filter = filter;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            (KeyCode::Char('A'), KeyModifiers::SHIFT) => {
                                let task_desc = self.input("Enter new task:", "").await?;
                                if !task_desc.is_empty() {
                                    let timeline = self.get_timeline().await?;
                                    let _added = self
                                        .api_cache
                                        .add_task(&timeline, &task_desc, None, None, None, true)
                                        .await?;
                                    self.update_tasks().await?;
                                }
                                StepResult::Cont
                            }
                            (KeyCode::Char('L'), KeyModifiers::SHIFT) => {
                                self.update_lists().await?;
                                StepResult::Cont
                            }
                            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                                self.ui_state.lock().await.refresh = true;
                                StepResult::Cont
                            }
                            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                                let api_cache = self.api_cache.clone();
                                let event_tx = self.ui_state.lock().await.event_tx.clone();
                                UiState::start_progress(
                                    &self.ui_state,
                                    "syncing...",
                                    &["|", "/", "-", "\\"],
                                    async move {
                                        api_cache.sync().await.unwrap();
                                        event_tx.send(TuiEvent::SyncFinished).await.unwrap();
                                    },
                                )
                                .await?;
                                StepResult::Cont
                            }
                            (KeyCode::Enter, KeyModifiers::NONE) => {
                                let mut ui_state = self.ui_state.lock().await;
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
                            (KeyCode::Char('C'), KeyModifiers::SHIFT) => {
                                let display_mode = self.ui_state.lock().await.display_mode;
                                match display_mode {
                                    DisplayMode::Tasks => {
                                        info!("Marking task as complete");
                                        self.for_each_selected(
                                            async |api_cache, tl, list, ts, task| {
                                                let resp = api_cache
                                                    .mark_complete_id(tl, list, &ts.id, &task.id)
                                                    .await?;
                                                if let Some(transaction) = resp {
                                                    if transaction.undoable
                                                        && !transaction.id.is_empty()
                                                    {
                                                        Ok(Some(transaction.id))
                                                    } else {
                                                        Ok(None)
                                                    }
                                                } else {
                                                    Ok(None)
                                                }
                                            },
                                        )
                                        .await?;
                                        info!("Marked as complete!");
                                        self.update_tasks().await?;
                                    }
                                    DisplayMode::Lists => {}
                                }
                                StepResult::Cont
                            }
                            (KeyCode::Char('U'), KeyModifiers::SHIFT) => {
                                self.undo_latest().await?;
                                self.update_tasks().await?;
                                StepResult::Cont
                            }
                            (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
                                let mut ui_state = self.ui_state.lock().await;
                                let ui_state = &mut *ui_state;
                                ui_state.list_pos = ui_state.list_pos.saturating_sub(1);
                                ui_state.tree_state.key_up();
                                StepResult::Cont
                            }
                            (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
                                let mut ui_state = self.ui_state.lock().await;
                                let ui_state = &mut *ui_state;
                                if ui_state.list_pos + 1 < ui_state.tree_items.len() {
                                    ui_state.list_pos += 1;
                                }
                                ui_state.tree_state.key_down();
                                StepResult::Cont
                            }
                            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                                let mut ui_state = self.ui_state.lock().await;
                                let ui_state = &mut *ui_state;
                                ui_state.tree_state.toggle_selected();
                                StepResult::Cont
                            }
                            (KeyCode::Char('?'), KeyModifiers::SHIFT | KeyModifiers::NONE)
                            | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                                let mut ui_state = self.ui_state.lock().await;
                                ui_state.show_help = !ui_state.show_help;
                                StepResult::Cont
                            }
                            _ => StepResult::Cont,
                        },
                        _ => StepResult::Cont,
                    }
                }
            },
            Some(TuiEvent::StateChanged) => {
                let display_mode = self.ui_state.lock().await.display_mode;
                match display_mode {
                    DisplayMode::Tasks => {}
                    DisplayMode::Lists => {
                        self.update_list_display().await?;
                    }
                }
                StepResult::Cont
            }
            Some(TuiEvent::Tick) => {
                self.ui_state.lock().await.tick();
                StepResult::Cont
            }
            Some(TuiEvent::SyncFinished) => {
                self.update_tasks().await.unwrap();
                StepResult::Cont
            }
            Some(TuiEvent::ListSyncFinished) => StepResult::Cont,
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

    async fn get_timeline(&mut self) -> Result<RTMTimeline, anyhow::Error> {
        if self.current_timeline.is_none() {
            self.current_timeline = Some(self.api_cache.get_timeline().await?);
            self.transactions.clear();
        }
        Ok(self.current_timeline.as_ref().unwrap().clone())
    }

    // The callback returns an optional transaction id for undo.
    async fn for_each_selected<F>(&mut self, f: F) -> Result<(), anyhow::Error>
    where
        F: AsyncFn(
            &TaskCache,
            &RTMTimeline,
            &str, // list id
            &TaskSeries,
            &Task,
        ) -> Result<Option<String>, anyhow::Error>,
    {
        let timeline = self.get_timeline().await?;

        let ui_state = self.ui_state.lock().await;
        let tree_pos = ui_state.tree_state.selected();
        let tinfo = &ui_state.flat_tasks[*tree_pos.last().unwrap()];
        let task = &tinfo.ts.task[0];
        if let Some(tid) = f(&self.api_cache, &timeline, &tinfo.list_id, &tinfo.ts, task).await? {
            self.transactions.push(tid);
        }

        Ok(())
    }

    async fn undo_latest(&mut self) -> Result<(), anyhow::Error> {
        if let Some(tl) = &self.current_timeline {
            if let Some(transaction) = self.transactions.pop() {
                self.api_cache.undo_transaction(tl, &transaction).await?;
            }
        }
        Ok(())
    }

    async fn add_missing_children(&self, tasks: RTMTasks) -> Result<RTMTasks, anyhow::Error> {
        use std::collections::hash_map::Entry;
        let mut all_lists = HashMap::new();

        let update_lists = |all_lists: &mut HashMap<String, HashMap<String, TaskSeries>>,
                            tasks: RTMTasks| {
            // Initially import the tasks into hash maps.
            for list in tasks.list {
                let list_map = all_lists.entry(list.id).or_default();
                if let Some(tss) = list.taskseries {
                    for ts in tss {
                        match list_map.entry(ts.id.clone()) {
                            Entry::Occupied(mut occupied_entry) => {
                                let cur_ts = occupied_entry.get_mut();
                                for new_t in ts.task {
                                    if !cur_ts.task.iter().any(|t| t.id == new_t.id) {
                                        cur_ts.task.push(new_t);
                                    }
                                }
                            }
                            Entry::Vacant(vacant_entry) => {
                                vacant_entry.insert(ts);
                            }
                        }
                    }
                }
            }
        };
        update_lists(&mut all_lists, tasks);

        let mut task_ids = Vec::new();
        for (_, l) in all_lists.iter() {
            for (_, ts) in l.iter() {
                for t in &ts.task {
                    task_ids.push(t.id.clone());
                }
            }
        }

        while let Some(id) = task_ids.pop() {
            let children = self.api_cache.get_task_children(&id).await?;
            for (_list, ts) in RtmTaskListIterator::new(&children) {
                for t in &ts.task {
                    task_ids.push(t.id.clone());
                }
            }

            update_lists(&mut all_lists, children);
        }

        // Now combine back into an RTMLists.
        let mut result: RTMTasks = Default::default();
        for (list_id, list) in all_lists.into_iter() {
            let new_list = RTMLists {
                id: list_id,
                taskseries: Some(list.into_values().collect()),
            };
            result.list.push(new_list);
        }
        Ok(result)
    }
}

async fn get_tasks(
    api_cache: &rememberthemilk::cache::TaskCache,
    filter: &str,
    id: &str,
) -> Result<RTMTasks, anyhow::Error> {
    info!("get_tasks({filter}, {id})");
    let tasks = api_cache.get_tasks_in_list(id, filter).await?;
    info!("get_tasks({filter}, {id}): got {} tasks", tasks.list.len());
    Ok(tasks)
}

impl Drop for Tui {
    fn drop(&mut self) {
        disable_raw_mode().unwrap();
        self.terminal.show_cursor().unwrap();
    }
}

pub async fn tui() -> Result<ExitCode, anyhow::Error> {
    info!("Creating new tui...");
    let mut tui = Tui::new().await?;

    info!("Running tui...");
    tui.run().await?;
    info!("Tui finished.");

    Ok(ExitCode::SUCCESS)
}
