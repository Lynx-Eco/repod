use std::path::Path;
use walkdir::WalkDir;
use anyhow::Result;
use std::collections::HashMap;

pub struct DirectoryTree {
    name: String,
    children: Vec<DirectoryTree>,
    is_file: bool,
}

impl DirectoryTree {
    pub fn build(path: &Path, excluded_patterns: &[&str]) -> Result<DirectoryTree> {
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

        // Collect all entries
        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| {
                !excluded_patterns
                    .iter()
                    .any(|pattern| e.path().to_string_lossy().contains(pattern))
            })
            .filter_map(Result::ok) {
            let path_str = entry.path().to_string_lossy().to_string();
            let parent_str = entry.path().parent().unwrap().to_string_lossy().to_string();
            let name = entry.file_name().to_string_lossy().to_string();

            let node = DirectoryTree {
                name,
                children: Vec::new(),
                is_file: entry.file_type().is_file(),
            };

            path_map.entry(parent_str).or_default().push(node);
        }

        // Build the tree recursively starting from root
        root.build_recursive(path, &mut path_map);
        root.sort_children();

        Ok(root)
    }

    fn build_recursive(
        &mut self,
        current_path: &Path,
        path_map: &mut HashMap<String, Vec<DirectoryTree>>
    ) {
        let current_path_str = current_path.to_string_lossy().to_string();
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

    fn sort_children(&mut self) {
        // Sort directories first, then files, both alphabetically
        self.children.sort_by(|a, b| {
            match (a.is_file, b.is_file) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => a.name.cmp(&b.name),
            }
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
                (format!("{}└── ", child_prefix), format!("{}    ", child_prefix))
            } else {
                (format!("{}├── ", child_prefix), format!("{}│   ", child_prefix))
            };

            child.format_with_prefix(&next_prefix, &next_child_prefix, output);
        }
    }
}
