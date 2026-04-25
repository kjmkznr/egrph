use clap::Parser;
use egrph_core::{CypherValue, InMemoryGraph, PropertyValue, QueryResult};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::collections::BTreeSet;
use std::time::Instant;

const PROMPT_PRIMARY: &str = "D ";
const PROMPT_CONTINUE: &str = "  ";
const HISTORY_FILE: &str = ".egrph_history";

const HELP_TEXT: &str = "\
Commands:
  .help              Show this help message
  .quit  or  .exit   Exit the shell
  .timer [on|off]    Toggle query execution timer (default: on)
  .mode  [MODE]      Set output mode: table (default), csv, json, line
  .tables            List all node labels in the graph
  .stats             Show node and edge counts
  .export            Export the entire graph as Cypher CREATE statements

Cypher queries must end with a semicolon (;).
Multi-line input is supported — keep typing until you add ';'.

Examples:
  CREATE (:Person {name: 'Alice', age: 30});
  MATCH (n:Person) RETURN n.name, n.age ORDER BY n.name;
  MATCH (a)-[r]->(b) RETURN a, type(r), b LIMIT 10;";

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "egrph",
    about = "egrph graph database interactive shell",
    long_about = "egrph is a lightweight graph database that supports the Cypher query language.\n\nEnter '.help' for usage hints.\nQueries must end with a semicolon (;)."
)]
struct Cli {
    /// Execute a single Cypher query, print results, then exit
    #[arg(short = 'c', long = "command", value_name = "QUERY")]
    command: Option<String>,

    /// Read Cypher queries from FILE and execute them, then exit
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    file: Option<std::path::PathBuf>,

    /// Output format for query results
    #[arg(long = "mode", default_value = "table", value_name = "MODE")]
    mode: String,

    /// Disable the per-query execution timer
    #[arg(long = "no-timer")]
    no_timer: bool,
}

// ---------------------------------------------------------------------------
// Output mode
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum OutputMode {
    Table,
    Csv,
    Json,
    Line,
}

impl OutputMode {
    fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "table" => Some(Self::Table),
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            "line" => Some(Self::Line),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Line => "line",
        }
    }
}

// ---------------------------------------------------------------------------
// Shell state
// ---------------------------------------------------------------------------

struct Shell {
    graph: InMemoryGraph,
    mode: OutputMode,
    timer: bool,
}

impl Shell {
    fn new(mode: OutputMode, timer: bool) -> Self {
        Shell {
            graph: InMemoryGraph::new(),
            mode,
            timer,
        }
    }

    // -----------------------------------------------------------------------
    // Query execution
    // -----------------------------------------------------------------------

    /// Execute `query`, print results, and return `true` (or `false` on fatal
    /// error — currently always `true`; callers decide whether to keep going).
    fn run_query(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }

        let start = Instant::now();
        match self.graph.execute(query) {
            Ok(result) => {
                let elapsed = start.elapsed();
                let row_count = result.rows.len();
                self.print_result(&result);
                if self.timer {
                    println!("{} rows ({:.3}s)", row_count, elapsed.as_secs_f64());
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // Dot-command handling  (returns false → exit shell)
    // -----------------------------------------------------------------------

    fn handle_dot_command(&mut self, input: &str) -> bool {
        let trimmed = input.trim();
        let (cmd, rest) = trimmed
            .split_once(' ')
            .map(|(a, b)| (a, b.trim()))
            .unwrap_or((trimmed, ""));

        match cmd {
            ".help" | "\\?" => {
                println!("{HELP_TEXT}");
            }

            ".quit" | ".exit" | "\\q" => return false,

            ".timer" => match rest {
                "on" => {
                    self.timer = true;
                    println!("Timer: on");
                }
                "off" => {
                    self.timer = false;
                    println!("Timer: off");
                }
                _ => println!(
                    "Timer is currently {}",
                    if self.timer { "on" } else { "off" }
                ),
            },

            ".mode" => {
                if rest.is_empty() {
                    println!("Current mode: {}", self.mode.as_str());
                } else {
                    match OutputMode::parse(rest) {
                        Some(m) => {
                            self.mode = m;
                            println!("Output mode: {}", self.mode.as_str());
                        }
                        None => {
                            eprintln!("Unknown mode '{rest}'. Valid modes: table, csv, json, line")
                        }
                    }
                }
            }

            ".tables" => {
                if self.graph.node_count() == 0 {
                    println!("(no nodes)");
                } else {
                    match self
                        .graph
                        .execute("MATCH (n) RETURN DISTINCT labels(n) AS labels")
                    {
                        Ok(result) => {
                            let mut labels = BTreeSet::new();
                            for row in &result.rows {
                                if let Some(CypherValue::List(items)) = row.values.first() {
                                    for item in items {
                                        if let CypherValue::String(s) = item {
                                            labels.insert(s.clone());
                                        }
                                    }
                                }
                            }
                            if labels.is_empty() {
                                println!("(no labels)");
                            } else {
                                for label in labels {
                                    println!("{label}");
                                }
                            }
                        }
                        Err(e) => eprintln!("Error: {e}"),
                    }
                }
            }

            ".stats" => {
                println!("Nodes : {}", self.graph.node_count());
                println!("Edges : {}", self.graph.edge_count());
            }

            ".export" => {
                print!("{}", self.graph.export_cypher());
            }

            _ => eprintln!("Unknown command '{cmd}'. Type '.help' for usage."),
        }

        true
    }

    // -----------------------------------------------------------------------
    // Output formatters
    // -----------------------------------------------------------------------

    fn print_result(&self, result: &QueryResult) {
        match self.mode {
            OutputMode::Table => self.print_table(result),
            OutputMode::Csv => self.print_csv(result),
            OutputMode::Json => self.print_json(result),
            OutputMode::Line => self.print_line(result),
        }
    }

    fn print_table(&self, result: &QueryResult) {
        if result.columns.is_empty() {
            return;
        }

        // Pre-render all cells so we can measure column widths.
        let cell_rows: Vec<Vec<String>> = result
            .rows
            .iter()
            .map(|row| row.values.iter().map(cypher_to_display).collect())
            .collect();

        let mut widths: Vec<usize> = result.columns.iter().map(|c| c.len()).collect();
        for row in &cell_rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        let bar = |join: &str, left: &str, right: &str, mid: &str| {
            let parts: Vec<String> = widths.iter().map(|w| mid.repeat(w + 2)).collect();
            format!("{left}{}{right}", parts.join(join))
        };

        println!("{}", bar("┬", "┌", "┐", "─"));

        let header: Vec<String> = result
            .columns
            .iter()
            .zip(&widths)
            .map(|(col, w)| format!(" {col:<w$} "))
            .collect();
        println!("│{}│", header.join("│"));

        println!("{}", bar("┼", "├", "┤", "─"));

        for row in &cell_rows {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, w)| format!(" {cell:<w$} "))
                .collect();
            println!("│{}│", cells.join("│"));
        }

        println!("{}", bar("┴", "└", "┘", "─"));
    }

    fn print_csv(&self, result: &QueryResult) {
        if result.columns.is_empty() {
            return;
        }
        println!(
            "{}",
            result
                .columns
                .iter()
                .map(|c| csv_escape(c))
                .collect::<Vec<_>>()
                .join(",")
        );
        for row in &result.rows {
            let values: Vec<String> = row
                .values
                .iter()
                .map(|v| csv_escape(&cypher_to_display(v)))
                .collect();
            println!("{}", values.join(","));
        }
    }

    fn print_json(&self, result: &QueryResult) {
        if result.columns.is_empty() || result.rows.is_empty() {
            println!("[]");
            return;
        }
        println!("[");
        let last = result.rows.len() - 1;
        for (i, row) in result.rows.iter().enumerate() {
            let pairs: Vec<String> = result
                .columns
                .iter()
                .zip(row.values.iter())
                .map(|(col, val)| format!("    \"{col}\": {}", cypher_to_json(val)))
                .collect();
            let comma = if i < last { "," } else { "" };
            println!("  {{\n{}\n  }}{comma}", pairs.join(",\n"));
        }
        println!("]");
    }

    fn print_line(&self, result: &QueryResult) {
        let last = result.rows.len().saturating_sub(1);
        for (i, row) in result.rows.iter().enumerate() {
            for (col, val) in result.columns.iter().zip(row.values.iter()) {
                println!("{col} = {}", cypher_to_display(val));
            }
            if i < last {
                println!();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Value serialisation helpers
// ---------------------------------------------------------------------------

fn prop_to_display(v: &PropertyValue) -> String {
    match v {
        PropertyValue::String(s) => s.clone(),
        PropertyValue::Int(i) => i.to_string(),
        PropertyValue::Float(f) => f.to_string(),
        PropertyValue::Bool(b) => b.to_string(),
    }
}

fn cypher_to_display(value: &CypherValue) -> String {
    match value {
        CypherValue::Null => "NULL".to_string(),
        CypherValue::Boolean(b) => b.to_string(),
        CypherValue::Integer(i) => i.to_string(),
        CypherValue::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        CypherValue::String(s) => s.clone(),
        CypherValue::List(items) => {
            let parts: Vec<String> = items.iter().map(cypher_to_display).collect();
            format!("[{}]", parts.join(", "))
        }
        CypherValue::Map(map) => {
            let mut pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{k}: {}", cypher_to_display(v)))
                .collect();
            pairs.sort();
            format!("{{{}}}", pairs.join(", "))
        }
        CypherValue::Node(node) => {
            let labels = if node.labels.is_empty() {
                String::new()
            } else {
                format!(":{}", node.labels.join(":"))
            };
            let mut props: Vec<String> = node
                .properties
                .iter()
                .map(|(k, v)| format!("{k}: {}", prop_to_display(v)))
                .collect();
            props.sort();
            format!("({}{}  {{{}}})", node.id, labels, props.join(", "))
        }
        CypherValue::Relationship(edge) => {
            let mut props: Vec<String> = edge
                .properties
                .iter()
                .map(|(k, v)| format!("{k}: {}", prop_to_display(v)))
                .collect();
            props.sort();
            format!(
                "[{}->{}:{}  {{{}}}]",
                edge.src,
                edge.dst,
                edge.label,
                props.join(", ")
            )
        }
        CypherValue::Path(path) => {
            format!(
                "<path: {} nodes, {} rels>",
                path.nodes.len(),
                path.relationships.len()
            )
        }
        CypherValue::Date(d) => d.to_string(),
        CypherValue::Timestamp(ts) => ts.to_rfc3339(),
    }
}

fn cypher_to_json(value: &CypherValue) -> String {
    match value {
        CypherValue::Null => "null".to_string(),
        CypherValue::Boolean(b) => b.to_string(),
        CypherValue::Integer(i) => i.to_string(),
        CypherValue::Float(f) => format!("{f}"),
        CypherValue::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        CypherValue::List(items) => {
            let parts: Vec<String> = items.iter().map(cypher_to_json).collect();
            format!("[{}]", parts.join(", "))
        }
        CypherValue::Map(map) => {
            let mut pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("\"{k}\": {}", cypher_to_json(v)))
                .collect();
            pairs.sort();
            format!("{{{}}}", pairs.join(", "))
        }
        other => {
            let s = cypher_to_display(other);
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        }
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let mode = OutputMode::parse(&cli.mode).unwrap_or_else(|| {
        eprintln!("Unknown mode '{}'. Falling back to 'table'.", cli.mode);
        OutputMode::Table
    });

    let mut shell = Shell::new(mode, !cli.no_timer);

    // --command: run one query then exit
    if let Some(query) = cli.command {
        let query = query.trim_end_matches(';');
        shell.run_query(query);
        return;
    }

    // --file: run all queries in the file then exit
    if let Some(path) = cli.file {
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading '{}': {e}", path.display());
                std::process::exit(1);
            }
        };
        for raw in src.split(';') {
            let q = raw.trim();
            if !q.is_empty() {
                shell.run_query(q);
            }
        }
        return;
    }

    // Interactive REPL
    run_repl(shell);
}

fn run_repl(mut shell: Shell) {
    println!(
        "egrph v{} (in-memory)\nEnter '.help' for usage hints.",
        env!("CARGO_PKG_VERSION")
    );

    let mut rl = DefaultEditor::new().expect("failed to initialise readline");

    // Try to load history from the home directory.
    let history_path = dirs_next();
    if let Some(ref p) = history_path {
        let _ = rl.load_history(p);
    }

    let mut buf = String::new(); // accumulates a multi-line query

    loop {
        let prompt = if buf.is_empty() {
            PROMPT_PRIMARY
        } else {
            PROMPT_CONTINUE
        };

        match rl.readline(prompt) {
            Ok(line) => {
                let _ = rl.add_history_entry(&line);

                let trimmed = line.trim();

                // Dot-commands are always single-line
                if trimmed.starts_with('.') || trimmed.starts_with('\\') {
                    if !buf.is_empty() {
                        eprintln!("(discarding incomplete query)");
                        buf.clear();
                    }
                    if !shell.handle_dot_command(trimmed) {
                        break; // .quit / .exit
                    }
                    continue;
                }

                // Accumulate the line into the buffer
                if !buf.is_empty() {
                    buf.push(' ');
                }
                buf.push_str(trimmed);

                // Execute once a semicolon terminates the query
                if buf.trim_end().ends_with(';') {
                    let query = buf.trim().trim_end_matches(';').to_string();
                    buf.clear();
                    shell.run_query(&query);
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C → discard current input
                if !buf.is_empty() {
                    buf.clear();
                    println!("(query discarded)");
                } else {
                    println!("(use '.exit' or Ctrl-D to quit)");
                }
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D → exit
                break;
            }
            Err(e) => {
                eprintln!("Input error: {e}");
                break;
            }
        }
    }

    if let Some(ref p) = history_path {
        let _ = rl.save_history(p);
    }
    println!("Bye!");
}

/// Returns a path for the history file, or None if home cannot be determined.
fn dirs_next() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|mut p| {
            p.push(HISTORY_FILE);
            p
        })
}
