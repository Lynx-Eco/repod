use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::path::Path;

pub struct DirectoryTree {
    name: String,
    children: Vec<DirectoryTree>,
    is_file: bool,
}

impl DirectoryTree {
    pub fn build(
        path: &Path,
        exclude_set: Option<&GlobSet>,
        only_patterns: &[String],
        only_dirs: &[String],
    ) -> Result<DirectoryTree> {
        let root_name = path
            .file_name()
            .unwrap_or_else(|| path.as_os_str())
            .to_string_lossy()
            .to_string();

        let mut root = DirectoryTree {
            name: root_name,
            children: Vec::new(),
            is_file: false,
        };

        // Build a map of parent paths to their children
        let mut path_map: HashMap<String, Vec<DirectoryTree>> = HashMap::new();

        // Build only-globset for file inclusion
        let mut gs_builder = GlobSetBuilder::new();
        let mut added = 0usize;
        for d in only_dirs {
            let d = d.trim_matches('/');
            if !d.is_empty() {
                let pat = format!("{}/**", d);
                if let Ok(g) = Glob::new(&pat) {
                    gs_builder.add(g);
                    added += 1;
                }
            }
        }
        for p in only_patterns {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            let expanded = if p.contains('/') {
                p.to_string()
            } else {
                format!("**/{}", p)
            };
            if let Ok(g) = Glob::new(&expanded) {
                gs_builder.add(g);
                added += 1;
            }
        }
        let only_set: Option<GlobSet> = if added == 0 {
            None
        } else {
            gs_builder.build().ok()
        };

        // Build the walker with ignore support
        let mut walker_builder = WalkBuilder::new(path);
        walker_builder
            .hidden(false) // We'll handle hidden files ourselves
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(true);

        // Collect all entries
        for entry in walker_builder
            .build()
            .filter_map(Result::ok)
            .filter(|entry| {
                let entry_path = entry.path();

                // Skip the root directory itself
                if entry_path == path {
                    return false;
                }

                let rel = entry_path.strip_prefix(path).unwrap_or(entry_path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");

                // Check excluded patterns
                if let Some(set) = exclude_set {
                    if set.is_match(&rel_str) {
                        return false;
                    }
                }

                // Check if it's a hidden file/folder (starts with .)
                let is_hidden = entry_path.components().any(|component| {
                    if let std::path::Component::Normal(name) = component {
                        name.to_string_lossy().starts_with('.')
                    } else {
                        false
                    }
                });

                if is_hidden {
                    return false;
                }

                // Respect only globs for files (directories are kept; pruned later)
                if let Some(ref set) = only_set {
                    if let Ok(rel) = entry_path.strip_prefix(path) {
                        let rels = rel.to_string_lossy().replace('\\', "/");
                        let is_file = entry.file_type().map(|ft| ft.is_file()).unwrap_or(false);
                        if is_file && !set.is_match(rels) {
                            return false;
                        }
                    }
                }

                true
            })
        {
            let entry_path = entry.path();
            let parent_str = entry_path
                .parent()
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let name = entry.file_name().to_string_lossy().to_string();
            let is_file = entry.file_type().map(|ft| ft.is_file()).unwrap_or(false);

            let node = DirectoryTree {
                name,
                children: Vec::new(),
                is_file,
            };

            path_map.entry(parent_str).or_default().push(node);
        }

        // Build the tree recursively starting from root
        root.build_recursive(path, &mut path_map);

        // Prune empty directories if any inclusion filters are specified
        if only_set.is_some() {
            root.prune_empty_directories();
        }

        root.sort_children();

        Ok(root)
    }

    fn build_recursive(
        &mut self,
        current_path: &Path,
        path_map: &mut HashMap<String, Vec<DirectoryTree>>,
    ) {
        let current_path_str = current_path.to_string_lossy().replace('\\', "/");
        if let Some(children) = path_map.remove(&current_path_str) {
            for mut child in children {
                if !child.is_file {
                    // If it's a directory, recursively build its children
                    let child_path = current_path.join(&child.name);
                    child.build_recursive(&child_path, path_map);
                }
                self.children.push(child);
            }
        }
    }

    fn prune_empty_directories(&mut self) -> bool {
        if self.is_file {
            return true; // Files are always kept
        }

        // Recursively prune children and keep only non-empty ones
        self.children
            .retain_mut(|child| child.prune_empty_directories());

        // A directory is kept if it has any children (files or non-empty directories)
        !self.children.is_empty()
    }

    fn sort_children(&mut self) {
        // Sort directories first, then files, both alphabetically
        self.children.sort_by(|a, b| match (a.is_file, b.is_file) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a.name.cmp(&b.name),
        });

        // Recursively sort children
        for child in &mut self.children {
            child.sort_children();
        }
    }

    pub fn format(&self) -> String {
        let mut output = String::new();
        self.format_with_prefix("", "", &mut output);
        output
    }

    fn format_with_prefix(&self, prefix: &str, child_prefix: &str, output: &mut String) {
        // Add root
        output.push_str(&format!("{}{}\n", prefix, self.name));

        // Add children
        for (i, child) in self.children.iter().enumerate() {
            let is_last = i == self.children.len() - 1;
            let (next_prefix, next_child_prefix) = if is_last {
                (
                    format!("{}└── ", child_prefix),
                    format!("{}    ", child_prefix),
                )
            } else {
                (
                    format!("{}├── ", child_prefix),
                    format!("{}│   ", child_prefix),
                )
            };

            child.format_with_prefix(&next_prefix, &next_child_prefix, output);
        }
    }
}
