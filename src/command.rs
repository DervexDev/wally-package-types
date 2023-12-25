use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use full_moon::ast::LastStmt;

use crate::link_mutator::*;
use crate::require_parser::*;
use crate::sourcemap::*;

#[derive(Parser, Debug)]
#[clap(author, version, about)]
pub struct Command {
    /// Path to sourcemap
    #[clap(short, long, value_parser)]
    pub sourcemap: PathBuf,

    /// Path to packages
    #[clap(value_parser)]
    pub packages_folder: PathBuf,
}

fn find_node(root: &SourcemapNode, path: PathBuf) -> Option<Vec<&SourcemapNode>> {
    let mut stack = vec![vec![root]];

    while let Some(node_path) = stack.pop() {
        let node = node_path.last().unwrap();
        if node.file_paths.contains(&path.to_path_buf()) {
            return Some(node_path);
        }

        for child in &node.children {
            let mut path = node_path.clone();
            path.push(child);
            stack.push(path);
        }
    }

    None
}

fn lua_files_filter(path: &&PathBuf) -> bool {
    match path.extension() {
        Some(extension) => extension == "lua" || extension == "luau",
        None => false,
    }
}

/// Given a list of components (e.g., ['script', 'Parent', 'Example']), converts it to a file path
fn file_path_from_components(
    path: &Path,
    root: &SourcemapNode,
    path_components: Vec<String>,
) -> Result<PathBuf> {
    let mut iter = path_components.iter();
    let first_in_chain = iter.next().expect("No path components");
    assert!(first_in_chain == "script" || first_in_chain == "game");

    let mut node_path = if first_in_chain == "script" {
        find_node(root, path.canonicalize()?).expect("could not find node path")
    } else {
        vec![root]
    };

    for component in iter {
        if component == "Parent" {
            node_path.pop().expect("No parent available");
        } else {
            node_path.push(
                node_path
                    .last()
                    .unwrap()
                    .find_child(component.to_string())
                    .expect("unable to find child"),
            );
        }
    }

    let current = node_path.last().unwrap();
    let file_path = current
        .file_paths
        .iter()
        .find(lua_files_filter)
        .expect("No file path for require")
        .clone();
    println!(
        "Required file is {} [{}], located at {}",
        current.name,
        current.class_name,
        file_path.display()
    );

    Ok(file_path)
}

fn mutate_thunk(path: &Path, root: &SourcemapNode) -> Result<()> {
    println!("Mutating {}", path.display());

    // The entry should be a thunk
    let parsed_code = full_moon::parse(&std::fs::read_to_string(path)?)?;
    assert!(parsed_code.nodes().last_stmt().is_some());

    if let Some(LastStmt::Return(r#return)) = parsed_code.nodes().last_stmt() {
        let returned_expression = r#return.returns().iter().next().unwrap();
        let path_components =
            match_require(returned_expression).expect("could not resolve path for require");

        println!("Found require in format {}", path_components.join("/"));

        let file_path = file_path_from_components(path, root, path_components)?;
        let pass_through_contents = std::fs::read_to_string(file_path)?;
        let returns = r#return.returns().clone();
        let new_link_contents = mutate_link(parsed_code, returns, &pass_through_contents)?;

        match new_link_contents {
            MutateLinkResult::Changed(new_ast) => std::fs::write(path, full_moon::print(&new_ast))?,
            MutateLinkResult::Unchanged => (),
        };
    }

    Ok(())
}

fn handle_index_directory(path: &Path, root: &SourcemapNode) -> Result<()> {
    for package_entry in std::fs::read_dir(path)?.flatten() {
        for thunk in std::fs::read_dir(package_entry.path())?.flatten() {
            if thunk.file_type().unwrap().is_file() {
                mutate_thunk(&thunk.path(), root)?;
            }
        }
    }

    Ok(())
}

impl Command {
    pub fn run(&self) -> Result<()> {
        let sourcemap_contents = std::fs::read_to_string(&self.sourcemap)?;
        let mut sourcemap: SourcemapNode = serde_json::from_str(&sourcemap_contents)?;

        // Mutate the sourcemap so that all file paths are canonicalized for simplicity
        // And that they contain pointers to their parent
        mutate_sourcemap(&mut sourcemap);

        for entry in std::fs::read_dir(&self.packages_folder)?.flatten() {
            if entry.file_name() == "_Index" {
                handle_index_directory(&entry.path(), &sourcemap)?;
                continue;
            }

            mutate_thunk(&entry.path(), &sourcemap)?;
        }

        Ok(())
    }
}
