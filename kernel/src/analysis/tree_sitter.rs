use crate::model::analysis::{MatchNode, MatchNodeContext, TreeSitterNode};
use crate::model::common::{Language, Position};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use tree_sitter::QueryCursor;

fn get_tree_sitter_language(language: &Language) -> Option<tree_sitter::Language> {
    extern "C" {
        fn tree_sitter_python() -> tree_sitter::Language;
        fn tree_sitter_javascript() -> tree_sitter::Language;
        fn tree_sitter_tsx() -> tree_sitter::Language;
        fn tree_sitter_rust() -> tree_sitter::Language;
    }

    match language {
        Language::JavaScript => Some(unsafe { tree_sitter_javascript() }),
        Language::Python => Some(unsafe { tree_sitter_python() }),
        Language::Rust => Some(unsafe { tree_sitter_rust() }),
        Language::TypeScript => Some(unsafe { tree_sitter_tsx() }),
    }
}

// get the tree-sitter tree
pub fn get_tree(code: &str, language: &Language) -> Option<tree_sitter::Tree> {
    let mut tree_sitter_parser = tree_sitter::Parser::new();
    let tree_sitter_language = get_tree_sitter_language(language);
    tree_sitter_language.and_then(|ts_lang| {
        tree_sitter_parser.set_language(ts_lang).unwrap();
        tree_sitter_parser.parse(code, None)
    })
}

// build the query from tree-sitter
pub fn get_query(query_code: &str, language: &Language) -> Result<tree_sitter::Query> {
    let tree_sitter_language =
        get_tree_sitter_language(language).ok_or(anyhow!("no language defined"))?;
    Ok(tree_sitter::Query::new(tree_sitter_language, query_code)?)
}

// Get all the match nodes based on a query. For each match, we build a `MatchNode`
// object. This object is deserialized and this is what is passed to the visit function.
// This is the first argument of the visit function.
// This `MatchNode` must have the captures and captures_list attributes that contains
// the values of the captures for the match.
//
// Note that we also add the context to the node that consists of the code and variables.
pub fn get_query_nodes(
    tree: &tree_sitter::Tree,
    query: &tree_sitter::Query,
    filename: &str,
    code: &str,
    variables: &HashMap<String, String>,
) -> Vec<MatchNode> {
    let mut query_cursor = QueryCursor::new();
    let mut match_nodes: Vec<MatchNode> = vec![];

    let query_result = query_cursor.matches(query, tree.root_node(), code.as_bytes());

    for query_match in query_result {
        let mut captures: HashMap<String, TreeSitterNode> = HashMap::new();
        let mut captures_list: HashMap<String, Vec<TreeSitterNode>> = HashMap::new();
        for capture in query_match.captures.iter() {
            let capture_name_opt = query
                .capture_names()
                .get(usize::try_from(capture.index).unwrap());
            let node_opt = map_node(capture.node);

            if let (Some(capture_name), Some(node)) = (capture_name_opt, node_opt) {
                captures.insert(capture_name.to_string(), node.clone());
                if !captures_list.contains_key(capture_name) {
                    captures_list.insert(capture_name.to_string(), vec![]);
                }
                captures_list
                    .get_mut(capture_name)
                    .unwrap()
                    .push(node.clone());
            }
        }
        match_nodes.push(MatchNode {
            captures: captures.clone(),
            captures_list: captures_list.clone(),
            context: MatchNodeContext {
                code: Some(code.to_string()),
                filename: filename.to_string(),
                variables: variables.clone(),
            },
        })
    }
    match_nodes
}

// map a node from the tree-sitter representation into our own internal representation
// this is the representation that is passed to the JavaScript layer and how we represent
// or expose the node to the end-user.
//
// If this is NOT a named node, we do not return anything.
pub fn map_node(node: tree_sitter::Node) -> Option<TreeSitterNode> {
    let mut ts_cursor = node.walk();

    fn map_node_internal(cursor: &mut tree_sitter::TreeCursor) -> Option<TreeSitterNode> {
        // we do not map space, parenthesis and other non-named nodes.
        if !cursor.node().is_named() {
            return None;
        }

        // map all the children as we should
        let mut children: Vec<TreeSitterNode> = vec![];
        if cursor.goto_first_child() {
            loop {
                let maybe_child = map_node_internal(cursor);
                if let Some(child) = maybe_child {
                    children.push(child);
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }

        // finally, build the return value.
        let ts_node = TreeSitterNode {
            ast_type: cursor.node().kind().to_string(),
            start: Position {
                line: u32::try_from(cursor.node().range().start_point.row + 1).unwrap(),
                col: u32::try_from(cursor.node().range().start_point.column + 1).unwrap(),
            },
            end: Position {
                line: u32::try_from(cursor.node().range().end_point.row + 1).unwrap(),
                col: u32::try_from(cursor.node().range().end_point.column + 1).unwrap(),
            },
            field_name: cursor.field_name().map(|v| v.to_string()),
            children,
        };

        Some(ts_node)
    }
    map_node_internal(&mut ts_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_get_tree() {
        let source_code = r#"
arr = ["foo", "bar"];

def func():
   pass;"#;
        let t = get_tree(source_code, &Language::Python);
        assert!(t.is_some());
        assert_eq!("module", t.unwrap().root_node().kind());
    }

    #[test]
    fn test_map_node_simple() {
        let source_code = r#"
arr = ["foo", "bar"];

def func():
   pass;"#;
        let t = get_tree(source_code, &Language::Python);
        assert!(t.is_some());
        let tree_node = map_node(t.unwrap().root_node());
        assert!(tree_node.is_some());
        let root = tree_node.unwrap();
        assert_eq!(2, root.children.len());
        assert_eq!(
            "expression_statement",
            root.children.get(0).unwrap().ast_type
        );
        assert_eq!(
            "function_definition",
            root.children.get(1).unwrap().ast_type
        );
        assert!(root.children.get(1).unwrap().field_name.is_none());
        let function_definition = root.children.get(1).unwrap();
        assert_eq!(
            "name",
            function_definition
                .children
                .get(0)
                .unwrap()
                .field_name
                .clone()
                .unwrap()
        );
    }

    #[test]
    fn test_javascript_get_tree() {
        let source_code = r#"
function foo() {console.log("bar");}"#;
        let t = get_tree(source_code, &Language::JavaScript);
        assert!(t.is_some());
        assert_eq!("program", t.unwrap().root_node().kind());
    }

    #[test]
    fn test_typescript_get_tree() {
        let source_code = r#"
let myAdd = function (x: number, y: number): number {
  return x + y;
};
"#;
        let t = get_tree(source_code, &Language::TypeScript);
        assert!(t.is_some());
        assert_eq!("program", t.unwrap().root_node().kind());
    }

    #[test]
    fn test_rust_get_tree() {
        let source_code = r#"
fn foo(bar: String) -> String {
   return "foobar".to_string();
}
"#;
        let t = get_tree(source_code, &Language::Rust);
        assert!(t.is_some());
        assert_eq!("source_file", t.unwrap().root_node().kind());
    }

    // test the number of node we should retrieve when executing a rule
    #[test]
    fn test_get_query_nodes() {
        let q = r#"
(class_definition
  name: (identifier) @classname
  superclasses: (argument_list
    (identifier)+ @superclasses
  )
)
        "#;

        let c = r#"
 class myClass(Parent):
    def __init__(self):
        pass
        "#;

        let tree = get_tree(c, &Language::Python).unwrap();
        let query = get_query(q, &Language::Python).expect("query defined");
        let query_nodes = get_query_nodes(&tree, &query, "myfile.py", c, &HashMap::new());
        assert_eq!(query_nodes.len(), 1);
        let query_node = query_nodes.get(0).unwrap();
        assert_eq!(2, query_node.captures_list.len());
        assert_eq!(1, query_node.captures_list.get("classname").unwrap().len());
        assert_eq!(
            1,
            query_node.captures_list.get("superclasses").unwrap().len()
        );
        assert_eq!(2, query_node.captures.len());
        assert!(query_node.captures.contains_key("superclasses"));
        let superclasses = query_node.captures.get("superclasses").unwrap();
        assert_eq!(2, superclasses.start.line);
        assert_eq!(16, superclasses.start.col);
        assert_eq!(2, superclasses.end.line);
        assert_eq!(22, superclasses.end.col);
        assert_eq!("identifier", superclasses.ast_type);
        assert_eq!(None, superclasses.field_name);
        assert!(query_node.captures.contains_key("classname"));
    }
}