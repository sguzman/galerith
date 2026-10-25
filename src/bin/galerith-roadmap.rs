//! Keep roadmap, upstream module inventory and acceptance percentages consistent.
//! No Python, external generators or implicit claims of successful site parity.
use serde::Deserialize;
use std::{collections::BTreeSet, error::Error, fs, process};

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
                            return Err(
                                format!("accepted check lacks evidence: {}", task.id).into()
                            );
                        }
                    }
                }
                let actual =
                    (100 * passed + task.checks.len() as u32 / 2) / task.checks.len() as u32;
                if task.progress != actual {
                    return Err(format!(
                        "stale progress for {}: {} instead of {}",
                        task.id, task.progress, actual
                    )
                    .into());
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
        let milestone_progress: Vec<u32> = milestone
            .phases
            .iter()
            .flat_map(|phase| phase.tasks.iter().map(|task| task.progress))
            .collect();
        let total = average(&milestone_progress);
        lines.push(format!(
            "## {} {} — {} — `{} {}%`",
            milestone.emoji,
            milestone.id,
            milestone.name,
            bar(total),
            total
        ));
        lines.push(format!("**Acceptance gate:** {}", milestone.gate));
        lines.push(String::new());
        for phase in &milestone.phases {
            let values: Vec<u32> = phase.tasks.iter().map(|task| task.progress).collect();
            let percent = average(&values);
            lines.push(format!(
                "### {} {} — {} — `{} {}%`",
                phase.emoji,
                phase.id,
                phase.name,
                bar(percent),
                percent
            ));
            lines.push(String::new());
            for task in &phase.tasks {
                lines.push(format!(
                    "- {} **{}** {} — `{} {}%`",
                    task.emoji,
                    task.id,
                    task.name,
                    bar(task.progress),
                    task.progress
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
        if !matches!(
            module.status.as_str(),
            "not_ported" | "partial" | "complete"
        ) {
            return Err(format!("unrecognized port status for {}", module.path).into());
        }
    }
    let partial = matrix
        .modules
        .iter()
        .filter(|x| x.status == "partial")
        .count();
    let complete = matrix
        .modules
        .iter()
        .filter(|x| x.status == "complete")
        .count();
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
            module
                .rust_paths
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
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
            return Err(format!(
                "{path} is stale: regenerate with cargo run --bin galerith-roadmap"
            )
            .into());
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

/// Keep the public README task percentage and module counts synchronized.
fn render_readme_progress(
    current: &str,
    ledger: &Ledger,
    matrix: &PortMatrix,
) -> Result<String, Box<dyn Error>> {
    let task_progress: Vec<u32> = ledger
        .milestones
        .iter()
        .flat_map(|m| {
            m.phases
                .iter()
                .flat_map(|p| p.tasks.iter().map(|t| t.progress))
        })
        .collect();
    let overall = average(&task_progress);
    let complete = matrix
        .modules
        .iter()
        .filter(|m| m.status == "complete")
        .count();
    let partial = matrix
        .modules
        .iter()
        .filter(|m| m.status == "partial")
        .count();
    let missing = matrix.modules.len() - complete - partial;
    let expected_progress = format!(
        "**Overall MVP:** \u{60}{} {}%\u{60} ({} acceptance-tracked tasks). This is task acceptance progress, **not** site coverage or feature parity.",
        bar(overall), overall, task_progress.len()
    );
    let expected_catalog = format!(
        "**Pinned extractor-source inventory:** {complete} complete · {partial} partial · {missing} not ported ({} upstream Python files).",
        matrix.modules.len()
    );
    let mut progress_count = 0;
    let mut catalog_count = 0;
    let lines: Vec<String> = current
        .lines()
        .map(|line| {
            if line.starts_with("**Overall MVP:** ") {
                progress_count += 1;
                expected_progress.clone()
            } else if line.starts_with("**Pinned extractor-source inventory:** ") {
                catalog_count += 1;
                expected_catalog.clone()
            } else {
                line.to_owned()
            }
        })
        .collect();
    if progress_count != 1 || catalog_count != 1 {
        return Err("README needs exactly one progress line and one source-inventory line".into());
    }
    let mut output = lines.join("\n");
    if current.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn execute() -> Result<(), Box<dyn Error>> {
    let check = std::env::args().any(|arg| arg == "--check");
    let ledger: Ledger = serde_json::from_str(&fs::read_to_string("docs/progress.json")?)?;
    let matrix: PortMatrix = serde_json::from_str(&fs::read_to_string("docs/port-matrix.json")?)?;
    verify_or_write("docs/ROADMAP.md", &render_roadmap(&ledger)?, check)?;
    verify_or_write("docs/PORT_MATRIX.md", &render_matrix(&matrix)?, check)?;
    let readme = fs::read_to_string("README.md")?;
    let expected_readme = render_readme_progress(&readme, &ledger, &matrix)?;
    verify_or_write("README.md", &expected_readme, check)?;
    Ok(())
}
