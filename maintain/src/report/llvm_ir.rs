use std::cmp::Reverse;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::MaintainResult;

#[derive(Debug, Clone, Default, Serialize)]
/// Aggregate operation counts attributed to one generated LLVM IR region.
pub struct OperationCounts {
    pub instructions: usize,
    pub loads: usize,
    pub stores: usize,
    pub aggregate_ops: usize,
    pub allocas: usize,
    pub calls: usize,
}

impl OperationCounts {
    fn record(&mut self, line: &str) {
        let opcode = line
            .trim()
            .split_once('=')
            .map_or_else(|| line.trim(), |(_, rest)| rest.trim())
            .split_whitespace()
            .next()
            .unwrap_or_default();
        self.instructions += 1;
        match opcode {
            "load" => self.loads += 1,
            "store" => self.stores += 1,
            "call" | "invoke" | "callbr" => self.calls += 1,
            "alloca" => self.allocas += 1,
            _ => {}
        }
        if [
            "extractvalue",
            "insertvalue",
            "getelementptr",
            "extractelement",
            "insertelement",
            "shufflevector",
            "memcpy",
            "memmove",
            "memset",
        ]
        .iter()
        .any(|operation| opcode == *operation || line.contains(&format!("llvm.{operation}")))
        {
            self.aggregate_ops += 1;
        }
    }

    fn add_assign(&mut self, other: &Self) {
        self.instructions += other.instructions;
        self.loads += other.loads;
        self.stores += other.stores;
        self.aggregate_ops += other.aggregate_ops;
        self.allocas += other.allocas;
        self.calls += other.calls;
    }
}

#[derive(Debug, Clone, Serialize)]
/// Function-level generated IR evidence, sorted by instruction count.
pub struct FunctionReport {
    pub symbol: String,
    pub debug_name: Option<String>,
    pub module: String,
    #[serde(skip)]
    module_index: usize,
    pub operations: OperationCounts,
}

#[derive(Debug, Clone, Serialize)]
/// Module-level generated IR evidence, sorted by instruction count.
pub struct ModuleReport {
    pub source_filename: String,
    pub lines: usize,
    pub bytes: usize,
    pub operations: OperationCounts,
}

#[derive(Debug, Clone, Serialize)]
/// Complete read-only attribution report for one concatenated LLVM IR file.
pub struct Report {
    pub input_bytes: usize,
    pub input_lines: usize,
    pub modules: Vec<ModuleReport>,
    pub functions: Vec<FunctionReport>,
}

#[derive(Debug, Default)]
struct ParsedModule {
    source_filename: String,
    lines: usize,
    bytes: usize,
    operations: OperationCounts,
}

#[derive(Debug)]
struct ParsedFunction {
    symbol: String,
    module_index: usize,
    module: String,
    operations: OperationCounts,
}

/// Reads, parses, and prints a generated LLVM IR attribution report.
pub fn run(input: &Path, limit: usize, json: bool) -> MaintainResult<()> {
    let source = fs::read_to_string(input)
        .map_err(|error| format!("failed to read LLVM IR {}: {error}", input.display()))?;
    let report = parse(&source);
    if json {
        let encoded = serde_json::to_string_pretty(&report)
            .map_err(|error| format!("failed to encode LLVM IR report: {error}"))?;
        println!("{encoded}");
    } else {
        print_text(&report, limit, input);
    }
    Ok(())
}

fn parse(source: &str) -> Report {
    let mut modules = Vec::new();
    let mut functions = Vec::new();
    let mut module = ParsedModule::default();
    let mut function = None::<ParsedFunction>;
    let mut line_count = 0usize;

    for raw_line in source.split_inclusive('\n') {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        line_count += 1;
        if line.starts_with("; ModuleID =") && (module.lines > 0 || !modules.is_empty()) {
            finish_function(&mut function, &mut functions);
            modules.push(ModuleReport {
                source_filename: module.source_filename,
                lines: module.lines,
                bytes: module.bytes,
                operations: module.operations,
            });
            module = ParsedModule::default();
        }
        module.lines += 1;
        module.bytes += raw_line.len();
        if let Some(filename) = line.strip_prefix("source_filename = ") {
            module.source_filename = quoted_value(filename).unwrap_or_else(|| filename.to_owned());
        }
        if let Some(current) = function.as_mut() {
            if line.trim() == "}" {
                finish_function(&mut function, &mut functions);
            } else if !is_label_or_ir_comment(line) {
                current.operations.record(line);
            }
        } else if line.trim_start().starts_with("define ")
            && let Some(symbol) = definition_symbol(line)
        {
            function = Some(ParsedFunction {
                symbol,
                module_index: modules.len(),
                module: module.source_filename.clone(),
                operations: OperationCounts::default(),
            });
        }
    }
    finish_function(&mut function, &mut functions);
    if module.lines > 0 {
        modules.push(ModuleReport {
            source_filename: module.source_filename,
            lines: module.lines,
            bytes: module.bytes,
            operations: module.operations,
        });
    }
    for item in &functions {
        if let Some(module) = modules.get_mut(item.module_index) {
            module.operations.add_assign(&item.operations);
        }
    }
    modules.sort_by_key(|module| Reverse(module.operations.instructions));
    functions.sort_by_key(|function| Reverse(function.operations.instructions));
    Report {
        input_bytes: source.len(),
        input_lines: line_count,
        modules,
        functions,
    }
}

fn finish_function(function: &mut Option<ParsedFunction>, functions: &mut Vec<FunctionReport>) {
    let Some(function) = function.take() else {
        return;
    };
    let debug_name =
        nia_mangle::demangle_stable_symbol(&function.symbol).map(|symbol| symbol.debug_name());
    functions.push(FunctionReport {
        symbol: function.symbol,
        debug_name,
        module: function.module,
        module_index: function.module_index,
        operations: function.operations,
    });
}

fn is_label_or_ir_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with(';') || trimmed.ends_with(':')
}

fn definition_symbol(line: &str) -> Option<String> {
    let at = line.find('@')? + 1;
    let rest = &line[at..];
    if let Some(rest) = rest.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_owned());
    }
    Some(
        rest.split(['(', ' ', '\t'])
            .next()
            .filter(|symbol| !symbol.is_empty())?
            .to_owned(),
    )
}

fn quoted_value(value: &str) -> Option<String> {
    let value = value.strip_prefix('"')?;
    let end = value.rfind('"')?;
    Some(value[..end].replace("\\22", "\"").replace("\\5C", "\\"))
}

fn print_text(report: &Report, limit: usize, input: &Path) {
    println!("llvm ir report: {}", input.display());
    println!(
        "input bytes={} lines={} modules={} functions={}",
        report.input_bytes,
        report.input_lines,
        report.modules.len(),
        report.functions.len()
    );
    println!("modules:");
    for (index, module) in report.modules.iter().take(limit).enumerate() {
        println!(
            "  {}. {} instructions={} lines={} bytes={} loads={} stores={} aggregate_ops={} allocas={} calls={}",
            index + 1,
            module.source_filename,
            module.operations.instructions,
            module.lines,
            module.bytes,
            module.operations.loads,
            module.operations.stores,
            module.operations.aggregate_ops,
            module.operations.allocas,
            module.operations.calls
        );
    }
    println!("functions:");
    for (index, function) in report.functions.iter().take(limit).enumerate() {
        let name = function.debug_name.as_deref().unwrap_or(&function.symbol);
        println!(
            "  {}. {} module={} symbol={} instructions={} loads={} stores={} aggregate_ops={} allocas={} calls={}",
            index + 1,
            name,
            function.module,
            function.symbol,
            function.operations.instructions,
            function.operations.loads,
            function.operations.stores,
            function.operations.aggregate_ops,
            function.operations.allocas,
            function.operations.calls
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_modules_and_demangles_canonical_functions() {
        let symbol = nia_mangle::mangle_stable_symbol(
            &nia_mangle::StableSymbolKey::new(
                "demo",
                nia_mangle::MangleModuleId::from_normalized_source_path("main.nia"),
                "0",
                "main",
                nia_mangle::MangleSymbolKind::Function,
                [],
            )
            .expect("valid symbol fixture"),
        );
        let ir = format!(
            "; ModuleID = 'a'\nsource_filename = \"a.nia\"\ndefine void @{symbol}() {{\nentry:\n  %p = alloca ptr\n  %v = load ptr, ptr %p\n  store ptr %v, ptr %p\n  ret void\n}}\n"
        );
        let report = parse(&ir);
        assert_eq!(report.modules.len(), 1);
        assert_eq!(report.functions.len(), 1);
        assert_eq!(report.functions[0].operations.instructions, 4);
        assert_eq!(report.functions[0].operations.loads, 1);
        assert_eq!(report.functions[0].operations.stores, 1);
        assert_eq!(
            report.functions[0].debug_name.as_deref(),
            Some("demo::main")
        );
        assert_eq!(report.modules[0].operations.instructions, 4);
    }

    #[test]
    fn separates_concatenated_modules() {
        let report = parse(
            "; ModuleID = 'a'\nsource_filename = \"a.nia\"\ndefine void @a() {\nentry:\n  ret void\n}\n; ModuleID = 'b'\nsource_filename = \"b.nia\"\ndefine void @b() {\nentry:\n  ret void\n}\n",
        );
        assert_eq!(report.modules.len(), 2);
        assert_eq!(report.functions.len(), 2);
        assert_eq!(report.modules[0].source_filename, "a.nia");
        assert_eq!(report.modules[1].source_filename, "b.nia");
    }
}
