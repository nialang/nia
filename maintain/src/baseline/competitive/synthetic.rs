use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use super::Language;
use crate::MaintainResult;

pub(super) const GENERATOR_IDENTITY: &str = "competitive_synthetic";
const MIXED_BRANCHES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GraphShape {
    FlatStar,
    DeepChain,
    MixedFanOut,
}

impl GraphShape {
    pub(super) const fn description(self) -> &'static str {
        match self {
            Self::FlatStar => "one executable root with a flat/star dependency on every module",
            Self::DeepChain => {
                "one executable root followed by a single dependency chain through every module"
            }
            Self::MixedFanOut => {
                "one executable root with ten equal-depth dependency-chain branches"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Specification {
    pub(super) module_count: usize,
    pub(super) graph_shape: GraphShape,
}

impl Specification {
    pub(super) const fn flat_star(module_count: usize) -> Self {
        Self {
            module_count,
            graph_shape: GraphShape::FlatStar,
        }
    }

    pub(super) const fn deep_chain(module_count: usize) -> Self {
        Self {
            module_count,
            graph_shape: GraphShape::DeepChain,
        }
    }

    pub(super) const fn mixed_fan_out(module_count: usize) -> Self {
        Self {
            module_count,
            graph_shape: GraphShape::MixedFanOut,
        }
    }
}

#[derive(Debug)]
struct ModuleNode {
    index: usize,
    parent: Option<usize>,
    children: Vec<usize>,
}

#[derive(Debug)]
struct ModuleGraph {
    modules: Vec<ModuleNode>,
    roots: Vec<usize>,
}

impl ModuleGraph {
    fn module_stem(&self, index: usize) -> PathBuf {
        let mut path = Vec::new();
        let mut current = Some(index);
        while let Some(index) = current {
            let module = &self.modules[index];
            path.push(module_name(module.index));
            current = module.parent;
        }
        path.reverse();
        path.into_iter().collect()
    }
}

fn module_graph(specification: Specification) -> MaintainResult<ModuleGraph> {
    let module_count = specification.module_count;
    if module_count == 0 {
        return Err("synthetic corpus requires at least one module".to_owned());
    }
    let branch_depth = match specification.graph_shape {
        GraphShape::MixedFanOut if !module_count.is_multiple_of(MIXED_BRANCHES) => {
            return Err(format!(
                "mixed-fan-out corpus requires a module count divisible by {MIXED_BRANCHES}"
            ));
        }
        GraphShape::MixedFanOut => module_count / MIXED_BRANCHES,
        GraphShape::FlatStar | GraphShape::DeepChain => 0,
    };
    let parents = (0..module_count)
        .map(|index| match specification.graph_shape {
            GraphShape::FlatStar => None,
            GraphShape::DeepChain => index.checked_sub(1),
            GraphShape::MixedFanOut if index % branch_depth == 0 => None,
            GraphShape::MixedFanOut => Some(index - 1),
        })
        .collect::<Vec<_>>();
    let mut modules = parents
        .iter()
        .enumerate()
        .map(|(index, parent)| ModuleNode {
            index,
            parent: *parent,
            children: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut roots = Vec::new();
    for (index, parent) in parents.into_iter().enumerate() {
        if let Some(parent) = parent {
            modules[parent].children.push(index);
        } else {
            roots.push(index);
        }
    }
    Ok(ModuleGraph { modules, roots })
}

pub(super) const fn expected_total(module_count: usize) -> u64 {
    let modules = module_count as u64;
    2 * modules * modules + 5 * modules
}

fn module_name(index: usize) -> String {
    format!("m{index:03}")
}

fn nia_module(index: usize, edited: bool, children: &[usize]) -> String {
    let adjustment = if edited { " + 1i64" } else { "" };
    let mut source = String::new();
    for child in children {
        writeln!(source, "pub module {};", module_name(*child)).expect("writing to String");
    }
    for child in children {
        writeln!(source, "using self::{};", module_name(*child)).expect("writing to String");
    }
    if !children.is_empty() {
        source.push('\n');
    }
    write!(
        source,
        "const INDEX: i64 = {index}i64;\n\
         const fn derive(seed: i64) i64 {{ seed * 3i64 + 7i64 }}\n\
         const DERIVED: i64 = derive(INDEX);\n\n\
         struct Marker {{\n    value: i64,\n}}\n\n\
         fn identity[T](value: T) T {{ value }}\n\n\
         pub fn value() i64 {{\n\
             let marker = identity(Marker {{ value: DERIVED + INDEX }});\n\
             marker.value{adjustment}"
    )
    .expect("writing to String");
    for child in children {
        write!(source, " + {}::value()", module_name(*child)).expect("writing to String");
    }
    source.push_str("\n}\n");
    source
}

fn rust_module(index: usize, edited: bool, children: &[usize]) -> String {
    let adjustment = if edited { " + 1" } else { "" };
    let mut source = String::new();
    for child in children {
        writeln!(source, "mod {};", module_name(*child)).expect("writing to String");
    }
    if !children.is_empty() {
        source.push('\n');
    }
    write!(
        source,
        "const INDEX: i64 = {index};\n\
         const fn derive(seed: i64) -> i64 {{ seed * 3 + 7 }}\n\
         const DERIVED: i64 = derive(INDEX);\n\n\
         struct Marker {{\n    value: i64,\n}}\n\n\
         fn identity<T>(value: T) -> T {{ value }}\n\n\
         pub fn value() -> i64 {{\n\
             identity(Marker {{ value: DERIVED + INDEX }}).value{adjustment}"
    )
    .expect("writing to String");
    for child in children {
        write!(source, " + {}::value()", module_name(*child)).expect("writing to String");
    }
    source.push_str("\n}\n");
    source
}

fn zig_module(index: usize, edited: bool, children: &[usize]) -> String {
    let adjustment = if edited { " + 1" } else { "" };
    let mut source = String::new();
    for child in children {
        let child = module_name(*child);
        writeln!(
            source,
            "const {child} = @import(\"{}/{child}.zig\");",
            module_name(index)
        )
        .expect("writing to String");
    }
    if !children.is_empty() {
        source.push('\n');
    }
    write!(
        source,
        "const index: i64 = {index};\n\
         fn derive(comptime seed: i64) i64 {{ return seed * 3 + 7; }}\n\
         const derived: i64 = derive(index);\n\n\
         const Marker = struct {{\n    value: i64,\n}};\n\n\
         fn identity(input: anytype) @TypeOf(input) {{ return input; }}\n\n\
         pub fn value() i64 {{\n\
             return identity(Marker{{ .value = derived + index }}).value{adjustment}"
    )
    .expect("writing to String");
    for child in children {
        write!(source, " + {}.value()", module_name(*child)).expect("writing to String");
    }
    source.push_str(";\n}\n");
    source
}

fn nia_root(graph: &ModuleGraph, accepts_leaf_edit: bool) -> String {
    let mut source = String::from("// Generated by competitive_synthetic.\n");
    for index in &graph.roots {
        writeln!(source, "pub module {};", module_name(*index)).expect("writing to String");
    }
    source.push('\n');
    for index in &graph.roots {
        writeln!(source, "using entry::{};", module_name(*index)).expect("writing to String");
    }
    source.push_str(
        "using std::io;\nusing std::process;\nusing process::{Init, ExitCode};\n\n\
         pub fn main(init: Init) ExitCode!() {\n\
             _ = init;\n\
             let mut total: i64 = 0i64;\n",
    );
    for index in &graph.roots {
        writeln!(source, "    total += {}::value();", module_name(*index))
            .expect("writing to String");
    }
    let total = expected_total(graph.modules.len());
    if accepts_leaf_edit {
        writeln!(source, "    if total == {total}i64 {{").expect("writing to String");
        source.push_str(
            "        io::debugPrint(&\"synthetic-ok\\n\", &[]).?;\n\
             return !();\n\
         }\n",
        );
        writeln!(
            source,
            "    if total != {}i64 {{ return ExitCode(1)!; }}",
            total + 1
        )
        .expect("writing to String");
        source.push_str("    io::debugPrint(&\"synthetic-edited\\n\", &[]).?;\n    !()\n}\n");
    } else {
        writeln!(
            source,
            "    if total != {total}i64 {{ return ExitCode(1)!; }}"
        )
        .expect("writing to String");
        source.push_str("    io::debugPrint(&\"synthetic-ok\\n\", &[]).?;\n    !()\n}\n");
    }
    source
}

fn rust_root(graph: &ModuleGraph, accepts_leaf_edit: bool) -> String {
    let mut source = String::from("// Generated by competitive_synthetic.\n");
    for index in &graph.roots {
        writeln!(source, "mod {};", module_name(*index)).expect("writing to String");
    }
    source.push_str("\nfn main() {\n    let mut total: i64 = 0;\n");
    for index in &graph.roots {
        writeln!(source, "    total += {}::value();", module_name(*index))
            .expect("writing to String");
    }
    let total = expected_total(graph.modules.len());
    if accepts_leaf_edit {
        writeln!(
            source,
            "    match total {{ {total} => println!(\"synthetic-ok\"), {} => println!(\"synthetic-edited\"), _ => panic!(\"invalid synthetic total\") }}",
            total + 1
        )
        .expect("writing to String");
        source.push_str("}\n");
    } else {
        writeln!(source, "    assert_eq!(total, {total});").expect("writing to String");
        source.push_str("    println!(\"synthetic-ok\");\n}\n");
    }
    source
}

fn zig_root(graph: &ModuleGraph, accepts_leaf_edit: bool) -> String {
    let mut source =
        String::from("// Generated by competitive_synthetic.\nconst std = @import(\"std\");\n");
    for index in &graph.roots {
        let name = module_name(*index);
        writeln!(source, "const {name} = @import(\"{name}.zig\");").expect("writing to String");
    }
    source.push_str("\npub fn main() !void {\n    var total: i64 = 0;\n");
    for index in &graph.roots {
        writeln!(source, "    total += {}.value();", module_name(*index))
            .expect("writing to String");
    }
    let total = expected_total(graph.modules.len());
    if accepts_leaf_edit {
        writeln!(
            source,
            "    const message = switch (total) {{ {total} => \"synthetic-ok\\n\", {} => \"synthetic-edited\\n\", else => return error.InvalidSyntheticTotal }};",
            total + 1
        )
        .expect("writing to String");
    } else {
        writeln!(
            source,
            "    if (total != {total}) return error.InvalidSyntheticTotal;"
        )
        .expect("writing to String");
        source.push_str("    const message = \"synthetic-ok\\n\";\n");
    }
    source.push_str(
        "    const io = std.Io.Threaded.global_single_threaded.io();\n\
             var buffer: [64]u8 = undefined;\n\
             var writer = std.Io.File.stdout().writer(io, &buffer);\n\
             try writer.interface.writeAll(message);\n\
             try writer.interface.flush();\n\
         }\n",
    );
    source
}

pub(super) fn generate(
    destination: &Path,
    language: Language,
    specification: Specification,
) -> MaintainResult<PathBuf> {
    write_sources(destination, language, specification, false)
}

fn write_sources(
    destination: &Path,
    language: Language,
    specification: Specification,
    accepts_leaf_edit: bool,
) -> MaintainResult<PathBuf> {
    let graph = module_graph(specification)?;
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let (extension, root) = match language {
        Language::Nia => ("nia", nia_root(&graph, accepts_leaf_edit)),
        Language::Rust => ("rs", rust_root(&graph, accepts_leaf_edit)),
        Language::Zig => ("zig", zig_root(&graph, accepts_leaf_edit)),
    };
    let entry = destination.join(format!("main.{extension}"));
    fs::write(&entry, root)
        .map_err(|error| format!("failed to write {}: {error}", entry.display()))?;
    for module in &graph.modules {
        let source = match language {
            Language::Nia => nia_module(module.index, false, &module.children),
            Language::Rust => rust_module(module.index, false, &module.children),
            Language::Zig => zig_module(module.index, false, &module.children),
        };
        let path = destination
            .join(graph.module_stem(module.index))
            .with_extension(extension);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&path, source)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(entry)
}

pub(super) fn generate_build_project(
    destination: &Path,
    language: Language,
    specification: Specification,
) -> MaintainResult<PathBuf> {
    let source_dir = destination.join("src");
    let entry = write_sources(&source_dir, language, specification, true)?;
    let (build_file, contents) = match language {
        Language::Nia => (
            "build.nia",
            "using std::build;\n\
             using std::fs;\n\n\
             pub fn build(b: &mut build::Build) build::Error!() {\n\
                 let rootModule = b.addModule(\n\
                     build::ModuleOptions::init(&\"synthetic\", fs::PathView::init(&\"src/main.nia\")),\n\
                 ).?;\n\
                 let executable = b.addExecutable(\n\
                     build::ExecutableOptions::init(&\"synthetic\", rootModule),\n\
                 ).?;\n\
                 let emit = b.addEmitExecutableStep(&\"synthetic\", executable).?;\n\
                 b.setDefaultStep(emit).?;\n\
                 !()\n\
             }\n"
                .to_owned(),
        ),
        Language::Rust => (
            "Cargo.toml",
            "[package]\n\
             name = \"competitive-synthetic\"\n\
             version = \"0.0.0\"\n\
             edition = \"2024\"\n\
             publish = false\n\n\
             [profile.dev]\n\
             debug = 0\n\n\
             [profile.release]\n\
             opt-level = 2\n\
             debug = 0\n"
                .to_owned(),
        ),
        Language::Zig => (
            "build.zig",
            "const std = @import(\"std\");\n\n\
             pub fn build(b: *std.Build) void {\n\
                 const target = b.standardTargetOptions(.{});\n\
                 const optimize = b.standardOptimizeOption(.{});\n\
                 const executable = b.addExecutable(.{\n\
                     .name = \"synthetic\",\n\
                     .root_module = b.createModule(.{\n\
                         .root_source_file = b.path(\"src/main.zig\"),\n\
                         .target = target,\n\
                         .optimize = optimize,\n\
                     }),\n\
                 });\n\
                 b.installArtifact(executable);\n\
             }\n"
                .to_owned(),
        ),
    };
    let path = destination.join(build_file);
    fs::write(&path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    if matches!(language, Language::Rust) {
        let lock = destination.join("Cargo.lock");
        fs::write(
            &lock,
            "# This file is automatically @generated by Cargo.\n\
             # It is not intended for manual editing.\n\
             version = 4\n\n\
             [[package]]\n\
             name = \"competitive-synthetic\"\n\
             version = \"0.0.0\"\n",
        )
        .map_err(|error| format!("failed to write {}: {error}", lock.display()))?;
    }
    Ok(entry)
}

pub(super) fn edit_leaf(
    project: &Path,
    language: Language,
    index: usize,
) -> MaintainResult<PathBuf> {
    let extension = match language {
        Language::Nia => "nia",
        Language::Rust => "rs",
        Language::Zig => "zig",
    };
    let relative = PathBuf::from("src").join(format!("{}.{extension}", module_name(index)));
    let source = match language {
        Language::Nia => nia_module(index, true, &[]),
        Language::Rust => rust_module(index, true, &[]),
        Language::Zig => zig_module(index, true, &[]),
    };
    let path = project.join(&relative);
    fs::write(&path, source)
        .map_err(|error| format!("failed to edit {}: {error}", path.display()))?;
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TemporaryDirectory;

    #[test]
    fn total_matches_generated_per_module_values() {
        for count in [1, 10, 50, 100, 500] {
            let actual = (0..count).map(|index| 4 * index as u64 + 7).sum::<u64>();
            assert_eq!(expected_total(count), actual);
        }
    }

    #[test]
    fn graph_shapes_preserve_module_and_edge_work() {
        let cases = [
            (Specification::flat_star(100), 100, 1),
            (Specification::deep_chain(100), 1, 100),
            (Specification::mixed_fan_out(100), 10, 10),
        ];
        for (specification, expected_roots, expected_depth) in cases {
            let graph = module_graph(specification).unwrap();
            let module_edges = graph
                .modules
                .iter()
                .map(|module| module.children.len())
                .sum::<usize>();
            assert_eq!(graph.modules.len(), 100);
            assert_eq!(graph.roots.len(), expected_roots);
            assert_eq!(module_edges + graph.roots.len(), 100);
            assert_eq!(
                graph
                    .modules
                    .iter()
                    .map(|module| graph.module_stem(module.index).components().count())
                    .max(),
                Some(expected_depth)
            );
        }
    }

    #[test]
    fn mixed_fan_out_rejects_an_ambiguous_branch_partition() {
        assert!(module_graph(Specification::mixed_fan_out(99)).is_err());
    }

    #[test]
    fn every_language_generates_one_root_and_the_requested_modules() {
        for language in [Language::Nia, Language::Rust, Language::Zig] {
            let temporary = TemporaryDirectory::new("nia-synthetic-test-").unwrap();
            let entry = generate(temporary.path(), language, Specification::flat_star(3)).unwrap();
            assert!(entry.is_file());
            assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 4);
            let root = fs::read_to_string(entry).unwrap();
            assert!(root.contains("competitive_synthetic"));
            assert!(root.contains("synthetic-ok"));
            assert!(root.contains("m002"));
            assert!(root.contains(&expected_total(3).to_string()));
        }
    }

    #[test]
    fn every_shape_generates_the_same_number_of_language_native_sources() {
        let specifications = [
            Specification::flat_star(100),
            Specification::deep_chain(100),
            Specification::mixed_fan_out(100),
        ];
        for specification in specifications {
            for language in [Language::Nia, Language::Rust, Language::Zig] {
                let temporary = TemporaryDirectory::new("nia-synthetic-graph-test-").unwrap();
                generate(temporary.path(), language, specification).unwrap();
                let graph = module_graph(specification).unwrap();
                assert!(graph.modules.iter().all(|module| {
                    temporary
                        .path()
                        .join(graph.module_stem(module.index))
                        .with_extension(language.extension())
                        .is_file()
                }));
            }
        }
    }

    #[test]
    fn build_projects_edit_exactly_one_leaf_without_touching_the_root() {
        for language in [Language::Nia, Language::Rust, Language::Zig] {
            let temporary = TemporaryDirectory::new("nia-synthetic-build-test-").unwrap();
            let entry =
                generate_build_project(temporary.path(), language, Specification::flat_star(3))
                    .unwrap();
            let root_before = fs::read_to_string(&entry).unwrap();
            let edited = edit_leaf(temporary.path(), language, 1).unwrap();
            assert_eq!(edited.parent(), Some(Path::new("src")));
            assert!(
                fs::read_to_string(temporary.path().join(edited))
                    .unwrap()
                    .contains("+ 1")
            );
            assert_eq!(fs::read_to_string(entry).unwrap(), root_before);
            assert!(
                temporary
                    .path()
                    .join("src/m000")
                    .with_extension(language.extension())
                    .is_file()
            );
        }
    }
}
