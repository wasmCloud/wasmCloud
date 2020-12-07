use crossterm::event::{
    read, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use log::{debug, error, info, trace, warn, LevelFilter};
use std::io::{self, Stdout};
use std::{cell::RefCell, io::Write, rc::Rc};
use structopt::{clap::AppSettings, StructOpt};
use tokio::runtime::*;
use tui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use tui_logger::*;
use wasmcloud_host::{Host, HostBuilder};

type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

const WASH_LOG_INFO: &str = "WASH_LOG";
const WASH_CMD_INFO: &str = "WASH_CMD";

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "up")]
pub struct UpCli {
    #[structopt(flatten)]
    command: UpCommand,
}

#[derive(StructOpt, Debug, Clone)]
struct UpCommand {
    /// Host for lattice connections, defaults to 0.0.0.0
    #[structopt(
        short = "h",
        long = "host",
        default_value = "0.0.0.0",
        env = "WASH_RPC_HOST"
    )]
    rpc_host: String,

    /// Port for lattice connections, defaults to 4222
    #[structopt(
        short = "p",
        long = "port",
        default_value = "4222",
        env = "WASH_RPC_PORT"
    )]
    rpc_port: String,
}

pub fn handle_command(cli: UpCli) -> Result<()> {
    match cli.command {
        UpCommand { .. } => handle_up(cli.command),
    }
}

#[derive(StructOpt, Debug, Clone)]
#[structopt(name = "wash>", global_settings(&[AppSettings::NoBinaryName, AppSettings::DisableVersion, AppSettings::ColorNever]))]
struct ReplCli {
    #[structopt(flatten)]
    cmd: ReplCliCommand,
}

#[derive(StructOpt, Debug, Clone)]
#[structopt(global_settings(&[AppSettings::ColorNever, AppSettings::DisableVersion, AppSettings::VersionlessSubcommands]))]
enum ReplCliCommand {
    /// Links an actor and a capability provider
    #[structopt(name = "link")]
    Link(LinkCommand),

    /// Invokes an operation on an actor
    #[structopt(name = "call")]
    Call(CallCommand),

    /// Starts an actor
    #[structopt(name = "start")]
    Start(StartCommand),

    /// Terminates the REPL environment (also accepts 'exit', 'logout', 'q' and ':q!')
    #[structopt(name = "quit", aliases = &["exit", "logout", "q", ":q!"])]
    Quit,
}

#[derive(StructOpt, Debug, Clone)]
struct LinkCommand {
    url: String,
    capid: String,
    env: Vec<String>,
}

#[derive(StructOpt, Debug, Clone)]
struct CallCommand {
    url: String,
    op: String,
    payload: Vec<String>,
}

#[derive(StructOpt, Debug, Clone)]
struct StartCommand {
    url: String,
}

#[derive(Debug, Clone)]
struct InputState {
    history: Vec<Vec<char>>,
    history_cursor: usize,
    input: Vec<char>,
    input_cursor: usize,
}

impl Default for InputState {
    fn default() -> Self {
        InputState {
            history: vec![],
            history_cursor: 0,
            input: vec![],
            input_cursor: 0,
        }
    }
}

impl InputState {
    fn cursor_location(&self, width: usize) -> (u16, u16) {
        let mut position = (0, 0);

        for current_char in self.input.iter().take(self.input_cursor) {
            let char_width = unicode_width::UnicodeWidthChar::width(*current_char).unwrap_or(0);

            position.0 += char_width;

            match position.0.cmp(&width) {
                std::cmp::Ordering::Equal => {
                    position.0 = 0;
                    position.1 += 1;
                }
                std::cmp::Ordering::Greater => {
                    // Handle a char with width > 1 at the end of the row
                    // width - (char_width - 1) accounts for the empty column(s) left behind
                    position.0 -= width - (char_width - 1);
                    position.1 += 1;
                }
                _ => (),
            }
        }

        (position.0 as u16, position.1 as u16)
    }
}

fn handle_key(state: &mut InputState, code: KeyCode, _modifiers: KeyModifiers) -> Result<()> {
    match code {
        KeyCode::Char(c) => {
            state.input.push(c);
            state.input_cursor += 1;
        }
        KeyCode::Left => {
            if state.input_cursor > 0 {
                state.input_cursor -= 1
            }
        }
        KeyCode::Right => {
            if state.input_cursor < state.input.len() {
                state.input_cursor += 1
            }
        }
        KeyCode::Backspace => {
            if state.input_cursor > 0 && state.input_cursor <= state.input.len() {
                state.input_cursor -= 1;
                state.input.remove(state.input_cursor);
            };
        }
        KeyCode::Enter => {
            let cmd: String = state.input.iter().collect();
            let iter = cmd.split_ascii_whitespace();
            let cli = ReplCli::from_iter_safe(iter);

            state.history.push(state.input.clone());
            state.history_cursor += 1;
            state.input.clear();
            state.input_cursor = 0;

            match cli {
                Ok(ReplCli { cmd }) => {
                    use ReplCliCommand::*;
                    match cmd {
                        Quit => {
                            info!(target: WASH_CMD_INFO, "Goodbye");
                            return Err("Quitting REPL".into());
                        }
                        Start(startcmd) => info!(
                            target: WASH_CMD_INFO,
                            "Parsed start command: {:?}", startcmd
                        ),
                        Link(linkcmd) => {
                            info!(target: WASH_CMD_INFO, "Parsed link command: {:?}", linkcmd)
                        }
                        Call(callcmd) => {
                            info!(target: WASH_CMD_INFO, "Parsed call command: {:?}", callcmd)
                        }
                    }
                }
                Err(e) => {
                    use structopt::clap::ErrorKind::*;
                    // HelpDisplayed is the StructOpt help text error, which should be displayed as info
                    match e.kind {
                        HelpDisplayed => info!(target: WASH_CMD_INFO, "\n{}", e.message),
                        _ => error!(target: WASH_CMD_INFO, "\n{}", e.message),
                    }
                }
            };
        }
        _ => (),
    };
    Ok(())
}

/// Launches REPL environment
fn handle_up(cmd: UpCommand) -> Result<()> {
    // Initialize logger at default level Trace
    init_logger(LevelFilter::Trace).unwrap();
    set_default_level(LevelFilter::Trace);

    // Initialize terminal
    let backend = {
        crossterm::terminal::enable_raw_mode().unwrap();
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture).unwrap();
        CrosstermBackend::new(stdout)
    };
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.clear().unwrap();
    terminal.hide_cursor().unwrap();

    // Initialize state and dispatcher, draw initial UI
    let mut state = InputState::default();
    let tui_state = TuiWidgetState::new();
    let dispatcher = Rc::new(RefCell::new(Dispatcher::<Event>::new()));
    info!(
        target: WASH_LOG_INFO,
        "=================================================================================="
    );
    info!(
        target: WASH_LOG_INFO,
        "Welcome to the WASH REPL! Your commands are being executed whenever you hit Return"
    );
    info!(
        target: WASH_LOG_INFO,
        "To see a list of commands, type 'help'. To exit the REPL, you can type 'quit'"
    );
    info!(
        target: WASH_LOG_INFO,
        "Press 'tab' to toggle between entering commands and changing log level"
    );
    info!(
        target: WASH_LOG_INFO,
        "The target selector supports arrow keys to navigate and alter log level"
    );
    info!(
        target: WASH_LOG_INFO,
        "as well as 'h' to hide the log selector"
    );
    info!(
        target: WASH_LOG_INFO,
        "=================================================================================="
    );
    draw_ui(&state, &mut terminal, &tui_state, &dispatcher)?;

    // let mut rt = Runtime::new()?;
    // let host = rt.block_on(launch_host(cmd.rpc_host, cmd.rpc_port))?;

    let mut repl_focus = true;
    loop {
        let res = match read()? {
            // Tab toggles input focus between REPL and Tui logger selector
            Event::Key(KeyEvent {
                code: KeyCode::Tab, ..
            }) => {
                repl_focus = !repl_focus;
                Ok(())
            }
            // Dispatch events for REPL interpretation
            Event::Key(KeyEvent { code, modifiers }) if repl_focus => {
                handle_key(&mut state, code, modifiers)
            }
            // Dispatch events for Tui Target interpretation
            evt => {
                dispatcher.borrow_mut().dispatch(&evt);
                Ok(())
            }
        };
        draw_ui(&state, &mut terminal, &tui_state, &dispatcher)?;

        // Exit the terminal gracefully
        if res.is_err() {
            terminal.show_cursor().unwrap();
            terminal.clear().unwrap();
            crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture).unwrap();
            terminal::disable_raw_mode().unwrap();
            break;
        }
    }
    Ok(())
}

// async fn launch_host(rpc_host: String, rpc_port: String) -> Result<Host> {
//     let nc = nats::asynk::connect(&format!("{}:{}", rpc_host, rpc_port)).await?;
//     let host = HostBuilder::new()
//         .with_rpc_client(nc)
//         .with_namespace("wasccrepl")
//         .build();
//     host.start().await.unwrap();

//     Ok(host)
// }

fn draw_ui(
    state: &InputState,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    tui_state: &TuiWidgetState,
    dispatcher: &Rc<RefCell<Dispatcher<Event>>>,
) -> Result<()> {
    terminal.draw(|frame| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Percentage(67)].as_ref())
            .split(frame.size());

        draw_input_panel(frame, state, chunks[0]);
        draw_selector_panel(frame, chunks[1], tui_state, &dispatcher);
    })?;
    Ok(())
}

/// Draws the waSH REPL in the provided frame
fn draw_input_panel(frame: &mut Frame<CrosstermBackend<Stdout>>, state: &InputState, chunk: Rect) {
    let history: String = state
        .history
        .iter()
        .map(|h| format!("wash> {}\n", h.iter().collect::<String>()))
        .collect();
    let prompt: String = "wash> ".to_string();
    let input = state.input.iter().collect::<String>();
    let display = format!("{}{}{}", history, prompt, input);

    // 3 is the offset from the bottom of the screen
    let scroll_offset = if state.history.len() as u16 >= chunk.height - 3 {
        state.history.len() as u16 + 3 - chunk.height
    } else {
        0
    };

    // Draw REPL panel
    let input_panel = Paragraph::new(display)
        .block(Block::default().borders(Borders::ALL).title(Span::styled(
            " REPL ",
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Left)
        .scroll((scroll_offset, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(input_panel, chunk);

    // Draw cursor on screen
    // Offset X by length of prompt plus current cursor
    // Offset Y by length of history (*2 for newlines)
    let x_offset = prompt.len() as u16;
    let input_cursor = state.cursor_location((chunk.width - 2) as usize);
    frame.set_cursor(
        chunk.x + 1 + input_cursor.0 + x_offset,
        chunk.y + 1 + input_cursor.1 + state.history.len() as u16 - scroll_offset,
    )
}

/// Draws the Tui smart logger widget in the provided frame
fn draw_selector_panel(
    frame: &mut Frame<CrosstermBackend<Stdout>>,
    chunk: Rect,
    state: &TuiWidgetState,
    dispatcher: &Rc<RefCell<Dispatcher<Event>>>,
) {
    dispatcher.borrow_mut().clear();
    let selector_panel = TuiLoggerSmartWidget::default()
        .style_error(Style::default().fg(Color::Red))
        .style_debug(Style::default().fg(Color::Green))
        .style_warn(Style::default().fg(Color::Yellow))
        .style_trace(Style::default().fg(Color::Magenta))
        .style_info(Style::default().fg(Color::Cyan))
        .state(state)
        .dispatcher(dispatcher.clone());
    // The tui_logger dispatcher on trace mode is extremely noisy (4 logs per keystroke)
    set_level_for_target("tui_logger::dispatcher", LevelFilter::Debug);

    frame.render_widget(selector_panel, chunk);
}
