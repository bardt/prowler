use crate::diff::FileDiff;

#[derive(Debug)]
pub enum TreeNode {
    Folder(FolderNode),
    File(FileNode),
}

#[derive(Debug)]
pub struct FolderNode {
    pub name: String,
    pub children: Vec<TreeNode>,
    pub collapsed: bool,
}

#[derive(Debug)]
pub struct FileNode {
    pub name: String,
    pub diff_idx: usize,
}

pub struct FileTree {
    roots: Vec<TreeNode>,
}

#[derive(Clone, Debug)]
pub struct VisibleRow {
    pub depth: usize,
    pub item: VisibleItem,
}

#[derive(Clone, Debug)]
pub enum VisibleItem {
    /// `path` indexes from `tree.roots` down through `Folder.children` to find this folder.
    Folder { path: Vec<usize>, name: String, collapsed: bool },
    File { diff_idx: usize, name: String },
}

impl FileTree {
    pub fn build(diffs: &[FileDiff]) -> Self {
        let mut tree = FileTree { roots: Vec::new() };
        for (idx, diff) in diffs.iter().enumerate() {
            let parts: Vec<&str> = diff.path.split('/').collect();
            insert_path(&mut tree.roots, &parts, idx);
        }
        sort_recursive(&mut tree.roots);
        tree
    }

    pub fn visible_rows(&self) -> Vec<VisibleRow> {
        let mut rows = Vec::new();
        let mut path = Vec::new();
        collect_visible(&self.roots, 0, &mut path, &mut rows);
        rows
    }

    /// Walk the tree to find the file leaf with the given `diff_idx`. Returns
    /// the index path from `roots` down through `Folder.children` to the leaf,
    /// or `None` if not found.
    pub fn find_file(&self, diff_idx: usize) -> Option<Vec<usize>> {
        fn walk(nodes: &[TreeNode], path: &mut Vec<usize>, target: usize) -> bool {
            for (i, node) in nodes.iter().enumerate() {
                path.push(i);
                match node {
                    TreeNode::File(f) if f.diff_idx == target => return true,
                    TreeNode::Folder(folder) => {
                        if walk(&folder.children, path, target) {
                            return true;
                        }
                    }
                    _ => {}
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        if walk(&self.roots, &mut path, diff_idx) {
            Some(path)
        } else {
            None
        }
    }

    pub fn folder_at_mut(&mut self, path: &[usize]) -> Option<&mut FolderNode> {
        let mut nodes: &mut Vec<TreeNode> = &mut self.roots;
        for (i, &idx) in path.iter().enumerate() {
            let node = nodes.get_mut(idx)?;
            let is_last = i + 1 == path.len();
            match node {
                TreeNode::Folder(f) => {
                    if is_last {
                        return Some(f);
                    }
                    nodes = &mut f.children;
                }
                TreeNode::File(_) => return None,
            }
        }
        None
    }
}

fn insert_path(nodes: &mut Vec<TreeNode>, parts: &[&str], diff_idx: usize) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        nodes.push(TreeNode::File(FileNode {
            name: parts[0].to_owned(),
            diff_idx,
        }));
        return;
    }
    let head = parts[0];
    let rest = &parts[1..];
    let existing = nodes
        .iter()
        .position(|n| matches!(n, TreeNode::Folder(f) if f.name == head));
    let folder_idx = match existing {
        Some(i) => i,
        None => {
            nodes.push(TreeNode::Folder(FolderNode {
                name: head.to_owned(),
                children: Vec::new(),
                collapsed: false,
            }));
            nodes.len() - 1
        }
    };
    if let TreeNode::Folder(f) = &mut nodes[folder_idx] {
        insert_path(&mut f.children, rest, diff_idx);
    }
}

fn sort_recursive(nodes: &mut Vec<TreeNode>) {
    nodes.sort_by(|a, b| {
        let (rank_a, name_a) = sort_key(a);
        let (rank_b, name_b) = sort_key(b);
        rank_a.cmp(&rank_b).then_with(|| name_a.cmp(name_b))
    });
    for node in nodes {
        if let TreeNode::Folder(f) = node {
            sort_recursive(&mut f.children);
        }
    }
}

fn sort_key(node: &TreeNode) -> (u8, &str) {
    match node {
        TreeNode::Folder(f) => (0, f.name.as_str()),
        TreeNode::File(f) => (1, f.name.as_str()),
    }
}

fn collect_visible(
    nodes: &[TreeNode],
    depth: usize,
    path: &mut Vec<usize>,
    out: &mut Vec<VisibleRow>,
) {
    for (idx, node) in nodes.iter().enumerate() {
        path.push(idx);
        match node {
            TreeNode::Folder(f) => {
                out.push(VisibleRow {
                    depth,
                    item: VisibleItem::Folder {
                        path: path.clone(),
                        name: f.name.clone(),
                        collapsed: f.collapsed,
                    },
                });
                if !f.collapsed {
                    collect_visible(&f.children, depth + 1, path, out);
                }
            }
            TreeNode::File(file) => {
                out.push(VisibleRow {
                    depth,
                    item: VisibleItem::File {
                        diff_idx: file.diff_idx,
                        name: file.name.clone(),
                    },
                });
            }
        }
        path.pop();
    }
}
