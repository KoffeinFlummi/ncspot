extern crate clap;
extern crate crossbeam_channel;
#[macro_use]
extern crate cursive;
extern crate directories;
extern crate failure;
extern crate futures;
extern crate librespot;
extern crate rspotify;
extern crate tokio;
extern crate tokio_core;
extern crate tokio_timer;
extern crate unicode_width;
extern crate webbrowser;

#[cfg(feature = "mpris")]
extern crate dbus;

#[macro_use]
extern crate serde;
extern crate serde_json;
extern crate toml;

#[macro_use]
extern crate log;
extern crate chrono;
extern crate fern;

extern crate rand;

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

#[cfg(target_os = "macos")]
extern crate cocoa;

use std::fs;
use std::process;
use std::sync::Arc;

use clap::{App, Arg};
use cursive::traits::Identifiable;
use cursive::Cursive;

use librespot::core::authentication::Credentials;

mod album;
mod artist;
mod authentication;
mod commands;
mod config;
mod events;
mod library;
mod playlist;
mod queue;
mod spotify;
mod theme;
mod track;
mod traits;
mod ui;

#[cfg(feature = "mpris")]
mod mpris;

#[cfg(target_os = "macos")]
mod macos;

use commands::CommandManager;
use events::{Event, EventManager};
use library::Library;
use spotify::PlayerEvent;

fn setup_logging(filename: &str) -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        // Perform allocation-free log formatting
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] [{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        // Add blanket level filter -
        .level(log::LevelFilter::Trace)
        // - and per-module overrides
        .level_for("librespot", log::LevelFilter::Debug)
        // Output to stdout, files, and other Dispatch configurations
        .chain(fern::log_file(filename)?)
        // Apply globally
        .apply()?;
    Ok(())
}

fn get_credentials(reset: bool) -> Credentials {
    let path = config::config_path("credentials.toml");
    if reset && fs::remove_file(&path).is_err() {
        error!("could not delete credential file");
    }

    let creds = ::config::load_or_generate_default(&path, authentication::create_credentials, true)
        .unwrap_or_else(|e| {
            eprintln!("{}", e);
            process::exit(1);
        });

    #[cfg(target_family = "unix")]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .unwrap_or_else(|e| {
            eprintln!("{}", e);
            process::exit(1);
        });

    creds
}

fn main() {
    let matches = App::new("ncspot")
        .version("0.1.0")
        .author("Henrik Friedrichsen <henrik@affekt.org> and contributors")
        .about("cross-platform ncurses Spotify client")
        .arg(
            Arg::with_name("debug")
                .short("d")
                .long("debug")
                .value_name("FILE")
                .help("Enable debug logging to the specified file")
                .takes_value(true),
        )
        .get_matches();

    if let Some(filename) = matches.value_of("debug") {
        setup_logging(filename).expect("can't setup logging");
    }

    // Things here may cause the process to abort; we must do them before creating curses windows
    // otherwise the error message will not be seen by a user
    let cfg: ::config::Config = {
        let path = config::config_path("config.toml");
        ::config::load_or_generate_default(path, |_| Ok(::config::Config::default()), false)
            .unwrap_or_else(|e| {
                eprintln!("{}", e);
                process::exit(1);
            })
    };

    let mut credentials = get_credentials(false);

    while !spotify::Spotify::test_credentials(credentials.clone()) {
        credentials = get_credentials(true);
    }

    let theme = theme::load(&cfg);

    let mut cursive = Cursive::default();
    cursive.set_theme(theme.clone());

    let event_manager = EventManager::new(cursive.cb_sink().clone());

    let spotify = Arc::new(spotify::Spotify::new(event_manager.clone(), credentials));

    let queue = Arc::new(queue::Queue::new(spotify.clone()));

    #[cfg(feature = "mpris")]
    let mpris_manager = Arc::new(mpris::MprisManager::new(spotify.clone(), queue.clone()));

    let library = Arc::new(Library::new(
        &event_manager,
        spotify.clone(),
        cfg.use_nerdfont.unwrap_or(false),
    ));

    let mut cmd_manager = CommandManager::new();
    cmd_manager.register_all(spotify.clone(), queue.clone(), library.clone());

    let cmd_manager = Arc::new(cmd_manager);
    CommandManager::register_keybindings(
        cmd_manager.clone(),
        &mut cursive,
        cfg.keybindings.clone(),
    );

    let search = ui::search::SearchView::new(
        event_manager.clone(),
        spotify.clone(),
        queue.clone(),
        library.clone(),
    );

    let libraryview = ui::library::LibraryView::new(queue.clone(), library.clone());

    let queueview = ui::queue::QueueView::new(queue.clone(), library.clone());

    let status = ui::statusbar::StatusBar::new(
        queue.clone(),
        spotify.clone(),
        cfg.use_nerdfont.unwrap_or(false),
    );

    let mut layout = ui::layout::Layout::new(status, &event_manager, theme)
        .view("search", search.with_id("search"), "Search")
        .view("library", libraryview.with_id("library"), "Library")
        .view("queue", queueview, "Queue");

    // initial view is queue
    layout.set_view("queue");

    cursive.add_global_callback(':', move |s| {
        s.call_on_id("main", |v: &mut ui::layout::Layout| {
            v.enable_cmdline();
        });
    });

    layout.cmdline.set_on_edit(move |s, cmd, _| {
        s.call_on_id("main", |v: &mut ui::layout::Layout| {
            if cmd.is_empty() {
                v.clear_cmdline();
            }
        });
    });

    {
        let ev = event_manager.clone();
        let cmd_manager = cmd_manager.clone();
        layout.cmdline.set_on_submit(move |s, cmd| {
            {
                let mut main = s.find_id::<ui::layout::Layout>("main").unwrap();
                main.clear_cmdline();
            }
            cmd_manager.handle(s, cmd.to_string()[1..].to_string());
            ev.trigger();
        });
    }

    cursive.add_fullscreen_layer(layout.with_id("main"));

    // cursive event loop
    while cursive.is_running() {
        cursive.step();
        for event in event_manager.msg_iter() {
            trace!("event received");
            match event {
                Event::Player(state) => {
                    if state == PlayerEvent::FinishedTrack {
                        queue.next(false);
                    }
                    spotify.update_status(state);

                    #[cfg(feature = "mpris")]
                    mpris_manager.update();
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(cmd) = macos::poll_macos_events() {
                cmd_manager.handle(&mut cursive, cmd);
            }
        }
    }
}
