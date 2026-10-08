//! Reject malformed graph edges before the JS reconstruction boundary.
use super::{MAX_NODES, Node, Result, StorageWireError, kinds};

pub(super) fn graph(nodes: &[Node], root: usize) -> Result<()> {
    for node in nodes {
        for edge in edges(node) {
            if edge >= nodes.len() {
                return Err(StorageWireError);
            }
        }
        match node {
            Node::Boxed(id)
                if !matches!(
                    nodes[*id],
                    Node::Boolean(_) | Node::Float(_) | Node::String(_) | Node::BigInt(_)
                ) =>
            {
                return Err(StorageWireError);
            }
            Node::Array { length, fields } => validate_array(*length, fields)?,
            Node::ArrayBuffer {
                data,
                max_byte_length: Some(max),
            } if *max < data.len() as u64 => {
                return Err(StorageWireError);
            }
            Node::DataView {
                buffer,
                byte_length,
                byte_offset,
            } => view(nodes, *buffer, *byte_offset, *byte_length, 1)?,
            Node::TypedArray {
                kind,
                buffer,
                byte_offset,
                length,
            } => view(
                nodes,
                *buffer,
                *byte_offset as u64,
                length.map(|v| v as u64),
                kinds::element_size(*kind) as u64,
            )?,
            _ => {}
        }
    }
    let mut colors = vec![0u8; nodes.len()];
    visit(nodes, root, 0, &mut colors)
}
fn visit(nodes: &[Node], id: usize, depth: usize, colors: &mut [u8]) -> Result<()> {
    if colors[id] != 0 {
        return Ok(());
    }
    if depth >= 64 {
        return Err(StorageWireError);
    }
    colors[id] = 1;
    for edge in edges(&nodes[id]) {
        visit(nodes, edge, depth + 1, colors)?;
    }
    colors[id] = 2;
    Ok(())
}
fn validate_array(length: u64, fields: &[(super::super::StringStore, usize)]) -> Result<()> {
    if length > MAX_NODES as u64 {
        return Err(StorageWireError);
    }
    let mut names = std::collections::HashSet::new();
    for (name, _) in fields {
        if !names.insert(name) {
            return Err(StorageWireError);
        }
        let text = String::from_utf16_lossy(&name.0);
        if text == "length" {
            return Err(StorageWireError);
        }
        if let Ok(index) = text.parse::<u32>()
            && index != u32::MAX
            && index.to_string() == text
            && u64::from(index) >= length
        {
            return Err(StorageWireError);
        }
    }
    Ok(())
}

fn view(nodes: &[Node], buffer: usize, offset: u64, length: Option<u64>, unit: u64) -> Result<()> {
    let Node::ArrayBuffer {
        data,
        max_byte_length,
    } = &nodes[buffer]
    else {
        return Err(StorageWireError);
    };
    if !offset.is_multiple_of(unit) {
        return Err(StorageWireError);
    }
    // Resizable buffers can legitimately preserve an out-of-bounds view.
    let bound = max_byte_length.unwrap_or(data.len() as u64);
    let end = offset
        .checked_add(
            length
                .unwrap_or(0)
                .checked_mul(unit)
                .ok_or(StorageWireError)?,
        )
        .ok_or(StorageWireError)?;
    if end > bound || length.is_none() && max_byte_length.is_none() {
        return Err(StorageWireError);
    }
    Ok(())
}
fn edges(node: &Node) -> Vec<usize> {
    match node {
        Node::Boxed(id) => vec![*id],
        Node::Object(fields) | Node::Array { fields, .. } => {
            fields.iter().map(|(_, id)| *id).collect()
        }
        Node::Map(entries) => entries.iter().flat_map(|(a, b)| [*a, *b]).collect(),
        Node::Set(entries) => entries.clone(),
        Node::Error { cause, .. } => cause.iter().copied().collect(),
        Node::DataView { buffer, .. } | Node::TypedArray { buffer, .. } => vec![*buffer],
        _ => vec![],
    }
}
