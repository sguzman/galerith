//! Keep roadmap, upstream module inventory and acceptance percentages consistent.
//! No Python, external generators or implicit claims of successful site parity.
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    process,
};

#[derive(Deserialize)]
struct Ledger {
    schema: u32,
    milestones: Vec<Milestone>,
}
#[derive(Deserialize)]
struct Milestone {
    id: String,
    emoji: String,
    name: String,
    gate: String,
    phases: Vec<Phase>,
}
#[derive(Deserialize)]
struct Phase {
    id: String,
    emoji: String,
    name: String,
    tasks: Vec<Task>,
}
#[derive(Deserialize)]
struct Task {
    id: String,
    emoji: String,
    name: String,
    progress: u32,
    checks: Vec<Check>,
}
#[derive(Deserialize)]
struct Check {
    title: String,
    passed: bool,
    evidence: Option<String>,
}
#[derive(Deserialize)]
struct PortMatrix {
    schema: u32,
    pinned_commit: String,
    module_count: usize,
    modules: Vec<Module>,
}
#[derive(Deserialize)]
struct Module {
    path: String,
    status: String,
    rust_paths: Vec<String>,
}

fn bar(n: u32) -> String {
    let filled = ((n + 5) / 10).min(10) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}
fn average(values: &[u32]) -> u32 {
    if values.is_empty() {
        return 0;
    }
    (values.iter().sum::<u32>() + (values.len() as u32 / 2)) / values.len() as u32
}

fn render_roadmap(ledger: &Ledger) -> Result<String, Box<dyn Error>> {
    if ledger.schema != 1 {
        return Err("unsupported progress ledger schema".into());
    }
    let mut ids = BTreeSet::new();
    let mut percentages = Vec::new();
    for milestone in &ledger.milestones {
        for phase in &milestone.phases {
            for task in &phase.tasks {
                if !ids.insert(task.id.as_str()) || task.checks.is_empty() {
                    return Err(format!("duplicate task ID or missing checks: {}", task.id).into());
                }
                let mut passed = 0_u32;
                for check in &task.checks {
                    if check.passed {
                        passed += 1;
                        if check.evidence.as_deref().unwrap_or("").trim().is_empty() {
                            return Err(format!("accepted check lacks evidence: {}", task.id).into());
                        }
                    }
                }
                let actual = (100 * passed + task.checks.len() as u32 / 2)
                    / task.checks.len() as u32;
                if task.progress != actual {
                    return Err(format!("stale progress for {}: {} instead of {}", task.id, task.progress, actual).into());
                }
                percentages.push(task.progress);
            }
        }
    }
    let overall = average(&percentages);
    let mut lines = vec![
        "# 🗺️ Galerith MVP roadmap".to_owned(),
        String::new(),
        "**Source of truth:** [progress.json](progress.json). Percentages reflect accepted atomic checks, not production readiness. No unverified CI or live site testing is counted.".to_owned(),
        String::new(),
        format!("**Overall:** `{} {}%` across {} atomic tasks. **Every milestone is part of the complete-port MVP.**", bar(overall), overall, percentages.len()),
        String::new(),
    ];
    for milestone in &ledger.milestones {
        let milestone_progress: Vec<u32> = milestone.phases.iter()
            .flat_map(|phase| phase.tasks.iter().map(|task| task.progress)).collect();
        let total = average(&milestone_progress);
        lines.push(format!(
            "## {} {} — {} — `{} {}%`",
            milestone.emoji, milestone.id, milestone.name, bar(total), total
        ));
        lines.push(format!("**Acceptance gate:** {}", milestone.gate));
        lines.push(String::new());
        for phase in &milestone.phases {
            let values: Vec<u32> = phase.tasks.iter().map(|task| task.progress).collect();
            let percent = average(&values);
            lines.push(format!(
                "### {} {} — {} — `{} {}%`",
                phase.emoji, phase.id, phase.name, bar(percent), percent
            ));
            lines.push(String::new());
            for task in &phase.tasks {
                lines.push(format!(
                    "- {} **{}** {} — `{} {}%`",
                    task.emoji, task.id, task.name, bar(task.progress), task.progress
                ));
                for check in &task.checks {
                    let evidence = if check.passed {
                        format!(" — {}", check.evidence.as_deref().unwrap_or(""))
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "  - [{}] {}{}",
                        if check.passed { "x" } else { " " },
                        check.title,
                        evidence
                    ));
                }
            }
            lines.push(String::new());
        }
    }
    lines.extend([
        "## 🏁 MVP condition".to_owned(),
        String::new(),
        "**Not reached.** Every milestone and every pinned catalog row must be complete with verification evidence. An intermediate working CLI is not an MVP.".to_owned(),
        String::new(),
    ]);
    Ok(lines.join("\n"))
}

fn render_matrix(matrix: &PortMatrix) -> Result<String, Box<dyn Error>> {
    if matrix.schema != 1 || matrix.modules.len() != matrix.module_count {
        return Err("invalid port matrix schema or catalog length".into());
    }
    let mut paths = BTreeSet::new();
    for module in &matrix.modules {
        if !paths.insert(module.path.as_str()) {
            return Err(format!("duplicate upstream path: {}", module.path).into());
        }
        if !matches!(module.status.as_str(), "not_ported" | "partial" | "complete") {
            return Err(format!("unrecognized port status for {}", module.path).into());
        }
    }
    let partial = matrix.modules.iter().filter(|x| x.status == "partial").count();
    let complete = matrix.modules.iter().filter(|x| x.status == "complete").count();
    let missing = matrix.modules.len() - partial - complete;
    let mut lines = vec![
        "# 🧩 Upstream extractor module parity matrix".to_owned(),
        String::new(),
        format!("**Reference:** [gallery-dl pinned commit](https://github.com/mikf/gallery-dl/commit/{}) · Machine data: [port-matrix.json](port-matrix.json).", matrix.pinned_commit),
        String::new(),
        format!("This is an inventory of **{} upstream Python source files** within `gallery_dl/extractor/`, including helper files and `__init__.py`, not a claim of {} separate sites. Individual modules may define multiple extractors, and the supported-site list is a separate audit.", matrix.modules.len(), matrix.modules.len()),
        String::new(),
        format!("**Complete: {complete} · Partial: {partial} · Not ported: {missing}.**"),
        String::new(),
        "| Upstream module | Status | Rust counterpart |".to_owned(),
        "|---|---|---|".to_owned(),
    ];
    for module in &matrix.modules {
        let counterpart = if module.rust_paths.is_empty() {
            "—".to_owned()
        } else {
            module.rust_paths.iter().map(|p| format!("`{p}`"))
                .collect::<Vec<_>>().join(", ")
        };
        lines.push(format!(
            "| `{}` | {} | {} |",
            module.path,
            match module.status.as_str() {
                "complete" => "🟢 Complete",
                "partial" => "🟡 Partial",
                _ => "⚪ Not ported",
            },
            counterpart
        ));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn verify_or_write(path: &str, expected: &str, check: bool) -> Result<(), Box<dyn Error>> {
    if check {
        let existing = fs::read_to_string(path)?;
        if existing != expected {
            return Err(format!("{path} is stale: regenerate with cargo run --bin galerith-roadmap").into());
        }
    } else {
        fs::write(path, expected)?;
        println!("updated {path}");
    }
    Ok(())
}

fn main() {
    if let Err(err) = execute() {
        eprintln!("galerith-roadmap: {err}");
        process::exit(1);
    }
}

fn execute() -> Result<(), Box<dyn Error>> {
    let check = std::env::args().any(|arg| arg == "--check");
    let ledger: Ledger = serde_json::from_str(&fs::read_to_string("docs/progress.json")?)?;
    let matrix: PortMatrix = serde_json::from_str(&fs::read_to_string("docs/port-matrix.json")?)?;
    verify_or_write("docs/ROADMAP.md", &render_roadmap(&ledger)?, check)?;
    verify_or_write("docs/PORT_MATRIX.md", &render_matrix(&matrix)?, check)?;
    Ok(())
}
