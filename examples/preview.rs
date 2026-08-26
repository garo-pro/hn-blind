//! Prints what hn-blind would announce, without opening a window.
//!
//! Useful for checking the API client and the wording of labels against live data: `cargo run --example preview -- top 5`
//!
//! `cargo run --example preview -- templates` instead lists every phrase the application can say, with its placeholders — the same set the settings dialog offers, in a form that fits in a terminal and a bug report.

use hn_blind::app::{App, View};
use hn_blind::config;
use hn_blind::hn::{Client, Feed};
use hn_blind::preferences;
use hn_blind::templates::{Template, Templates};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Loaded rather than defaulted, so this previews what *this* user hears.
    let (templates, note) = config::load();
    if let Some(note) = note {
        eprintln!("{note}");
    }
    let (preferences, note) = preferences::load();
    if let Some(note) = note {
        eprintln!("{note}");
    }

    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("templates") {
        print_templates(&templates);
        return Ok(());
    }

    let feed = match first.as_deref() {
        Some("new") => Feed::New,
        Some("best") => Feed::Best,
        Some("ask") => Feed::Ask,
        Some("show") => Feed::Show,
        Some("job") => Feed::Job,
        _ => Feed::Top,
    };
    let count: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(5);

    let client = Client::new();
    let mut app = App::new(feed, templates, preferences);

    let ids = client.story_ids(feed, count)?;
    app.stories = client.items(&ids);
    println!("== {} ==", app.list_title());
    for index in 0..app.row_count() {
        println!("{}", app.row_label(index));
    }

    // Then the first story's thread, as the comments view would read it.
    let Some(story) = app.stories.first().cloned() else {
        return Ok(());
    };
    if story.kids.is_empty() {
        return Ok(());
    }

    app.comments = client.comment_thread(&story.kids, 20);
    app.comment_story = Some(story);
    app.view = View::Comments;

    println!("\n== {} ==", app.list_title());
    for index in 0..app.row_count() {
        println!("{}", app.row_label(index));
    }

    // And the first comment as `P` would read it out: the one piece of wording that never appears in a row label.
    println!("\n== read in full ==\n{}", app.selected_detail());

    Ok(())
}

fn print_templates(templates: &Templates) {
    let mut group = None;

    for template in Template::ALL {
        if group != Some(template.group()) {
            group = Some(template.group());
            println!("\n== {} ==", template.group().name());
        }

        let mark = if templates.is_default(*template) {
            " "
        } else {
            "*"
        };
        println!("{mark} {} ({})", template.label(), template.id());
        println!("    {}", templates.get(*template));
        if !template.placeholders().is_empty() {
            println!("    placeholders: {}", template.placeholders().join(", "));
        }
    }

    let changed = templates.changed().count();
    println!(
        "\n{} templates, {changed} changed from the default{}",
        Template::ALL.len(),
        match config::path() {
            Some(path) => format!(", stored in {}", path.display()),
            None => String::new(),
        }
    );
}
