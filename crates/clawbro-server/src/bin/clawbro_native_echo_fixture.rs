use clawbro::runtime::{RuntimeEvent, RuntimeSessionSpec};
use std::io::{self, Read};

fn main() {
    if let Err(err) = run() {
        eprintln!("clawbro-native-echo-fixture: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let session: RuntimeSessionSpec = serde_json::from_str(&input)?;
    let text = session
        .context
        .user_input
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("fixture");
    let full = format!("native:{text}");
    println!(
        "{}",
        serde_json::to_string(&RuntimeEvent::TextDelta { text: full.clone() })?
    );
    println!(
        "{}",
        serde_json::to_string(&RuntimeEvent::TurnComplete { full_text: full })?
    );
    Ok(())
}
