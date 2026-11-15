//! Snapshot-bound inspection and source-preserving edits of authored Contract.

use contract_syntax::{Attr, Component, File, Lexer, Node, Span, Token, TokenKind, TypeExpr};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const INSPECT_USAGE: &str =
    "usage: contract inspect <file.contract> [--component <name>] [--tag <tag>] [--has <attr> …] [--where 'attr=expression' …] [--match <text> …] [--context] [--json]";
const EDIT_USAGE: &str = "usage: contract edit <file.contract> [--component <name>] [--tag <tag>] [--has <attr> …] [--where 'attr=expression' …] --match <text> --set|--or|--and 'name=expression'|--unset <name>|--text <plain-text> [--match …] [--apply] [--json] [--diagnostics]\n       contract edit <file.contract> --stdin [--apply] [--json] [--diagnostics]";

struct Problem {
    id: &'static str,
    message: String,
    diagnostics: Vec<Value>,
    candidates: Vec<Value>,
}

impl Problem {
    fn new(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            message: message.into(),
            diagnostics: Vec::new(),
            candidates: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Filters {
    has: Vec<String>,
    equals: Vec<(String, Vec<TokenKind>)>,
}

impl Filters {
    fn is_empty(&self) -> bool {
        self.has.is_empty() && self.equals.is_empty()
    }

    fn add(&mut self, flag: &str, text: &str) -> Result<(), Problem> {
        if flag == "--has" {
            self.has.push(text.to_string());
            return Ok(());
        }
        let (name, value) = text
            .split_once('=')
            .filter(|(name, _)| !name.trim().is_empty())
            .ok_or_else(|| Problem::new("selector-where", "--where requires attr=expression"))?;
        let value = expression(&json!({"value":value.trim()}))?;
        self.equals
            .push((name.trim().to_string(), expression_tokens(&value)?));
        Ok(())
    }

    fn matches(&self, source: &Source, node: &AuthoredNode) -> bool {
        self.has
            .iter()
            .all(|name| node.attributes.iter().any(|a| &a.name == name))
            && self.equals.iter().all(|(name, expected)| {
                node.attributes.iter().any(|a| {
                    &a.name == name
                        && expression_tokens(if a.bare {
                            "true"
                        } else {
                            &source.text[a.value_start..a.end]
                        })
                        .is_ok_and(|actual| &actual == expected)
                })
            })
    }
}

fn expression_tokens(text: &str) -> Result<Vec<TokenKind>, Problem> {
    Lexer::tokenize(text, 1)
        .map(|tokens| {
            tokens
                .into_iter()
                .filter(|t| !structural(t))
                .map(|t| t.kind)
                .collect()
        })
        .map_err(|e| Problem::new("selector-expression", e.message))
}

fn report(result: Result<Value, Problem>) -> ExitCode {
    match result {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            println!(
                "{}",
                json!({"ok":false,"applied":false,"error":{"id":problem.id,"message":problem.message,"candidates":problem.candidates},"diagnostics":problem.diagnostics})
            );
            ExitCode::from(1)
        }
    }
}

pub fn inspect(args: &[String]) -> ExitCode {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") {
        println!("{INSPECT_USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(path) = args.first().filter(|p| !p.starts_with('-')) else {
        eprintln!("{INSPECT_USAGE}");
        return ExitCode::from(2);
    };
    let (mut component, mut tag) = (None, None);
    let mut needles = Vec::new();
    let mut filters = Filters::default();
    let mut detailed = false;
    let mut context = false;
    let mut flags = args[1..].iter();
    while let Some(flag) = flags.next() {
        if flag == "--json" && !detailed {
            detailed = true;
            continue;
        }
        if flag == "--context" && !context {
            context = true;
            continue;
        }
        if flag == "--match" {
            let Some(value) = flags.next() else {
                eprintln!("{INSPECT_USAGE}");
                return ExitCode::from(2);
            };
            needles.push(value.as_str());
            continue;
        }
        if matches!(flag.as_str(), "--has" | "--where") {
            let Some(value) = flags.next() else {
                eprintln!("{INSPECT_USAGE}");
                return ExitCode::from(2);
            };
            if let Err(error) = filters.add(flag, value) {
                return report(Err(error));
            }
            continue;
        }
        let destination = match flag.as_str() {
            "--component" if component.is_none() => &mut component,
            "--tag" if tag.is_none() => &mut tag,
            _ => {
                eprintln!("{INSPECT_USAGE}");
                return ExitCode::from(2);
            }
        };
        let Some(value) = flags.next() else {
            eprintln!("{INSPECT_USAGE}");
            return ExitCode::from(2);
        };
        *destination = Some(value.as_str());
    }
    report(inspect_source(
        Path::new(path),
        component,
        tag,
        &needles,
        detailed,
        &filters,
        context,
    ))
}

struct Selection {
    needle: String,
    edits: Vec<Value>,
}

pub fn edit(args: &[String]) -> ExitCode {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") {
        println!("{EDIT_USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(path) = args.first().filter(|p| !p.starts_with('-')) else {
        eprintln!("{EDIT_USAGE}");
        return ExitCode::from(2);
    };
    let mut apply = false;
    let mut stdin = false;
    let mut diagnostics = false;
    let mut detailed = false;
    let mut component = None;
    let mut tag = None;
    let mut filters = Filters::default();
    let mut groups: Vec<Selection> = Vec::new();
    let mut flags = args[1..].iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--stdin" if !stdin => stdin = true,
            "--apply" if !apply => apply = true,
            "--diagnostics" if !diagnostics => diagnostics = true,
            "--json" if !detailed => detailed = true,
            "--has" | "--where" => {
                let Some(value) = flags.next() else {
                    eprintln!("{EDIT_USAGE}");
                    return ExitCode::from(2);
                };
                if let Err(error) = filters.add(flag, value) {
                    return report(Err(error));
                }
            }
            "--component" if component.is_none() => {
                let Some(name) = flags.next() else {
                    eprintln!("{EDIT_USAGE}");
                    return ExitCode::from(2);
                };
                component = Some(name.as_str());
            }
            "--tag" if tag.is_none() => {
                let Some(name) = flags.next() else {
                    eprintln!("{EDIT_USAGE}");
                    return ExitCode::from(2);
                };
                tag = Some(name.as_str());
            }
            "--match" => {
                let Some(needle) = flags.next() else {
                    eprintln!("{EDIT_USAGE}");
                    return ExitCode::from(2);
                };
                groups.push(Selection {
                    needle: needle.clone(),
                    edits: Vec::new(),
                });
            }
            "--set" | "--unset" | "--or" | "--and" | "--text" => {
                let (Some(value), Some(group)) = (flags.next(), groups.last_mut()) else {
                    eprintln!("{EDIT_USAGE}");
                    return ExitCode::from(2);
                };
                let operation = if flag == "--text" {
                    json!({"op":"set-text","value":value})
                } else if flag != "--unset" {
                    let Some((name, value)) = value.split_once('=') else {
                        eprintln!("{EDIT_USAGE}");
                        return ExitCode::from(2);
                    };
                    let op = match flag.as_str() {
                        "--or" => "or-attr",
                        "--and" => "and-attr",
                        _ => "set-attr",
                    };
                    json!({"op":op,"name":name.trim(),"value":value.trim()})
                } else {
                    json!({"op":"remove-attr","name":value})
                };
                group.edits.push(operation);
            }
            _ => {
                eprintln!("{EDIT_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if (stdin
        && (!groups.is_empty() || component.is_some() || tag.is_some() || !filters.is_empty()))
        || (!stdin && (groups.is_empty() || groups.iter().any(|g| g.edits.is_empty())))
    {
        eprintln!("{EDIT_USAGE}");
        return ExitCode::from(2);
    }
    if !stdin {
        return report(
            direct_edit(Path::new(path), component, tag, &groups, apply, &filters)
                .map(|receipt| format_receipt(receipt, detailed, diagnostics)),
        );
    }
    report((|| {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|e| Problem::new("edit-input", e.to_string()))?;
        let request: Value =
            serde_json::from_str(&input).map_err(|e| Problem::new("edit-input", e.to_string()))?;
        edit_source(Path::new(path), &request, apply)
            .map(|receipt| format_receipt(receipt, detailed, diagnostics))
    })())
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

struct Source {
    path: PathBuf,
    text: String,
    revision: String,
    file: File,
    tokens: Vec<Token>,
    starts: Vec<usize>,
}

impl Source {
    fn read(path: &Path) -> Result<Self, Problem> {
        let path = path
            .canonicalize()
            .map_err(|e| Problem::new("source-read", e.to_string()))?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Problem::new("source-read", e.to_string()))?;
        let file = contract_syntax::parse(&text).map_err(|e| Problem {
            id: "source-syntax",
            message: e.message.clone(),
            diagnostics: vec![json!({"id":e.id,"message":e.message,"line":e.span.line,"col":e.span.col,"end_col":e.span.end_col})],
            candidates:Vec::new(),
        })?;
        let tokens =
            Lexer::tokenize(&text, 1).map_err(|e| Problem::new("source-syntax", e.message))?;
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Ok(Self {
            path,
            revision: digest(&text),
            text,
            file,
            tokens,
            starts,
        })
    }

    fn offset(&self, span: Span, end: bool) -> usize {
        self.starts[span.line as usize - 1] + (if end { span.end_col } else { span.col }) as usize
            - 1
    }

    fn handle(&self, kind: &str, start: usize) -> String {
        format!("{kind}{}-{start}", &self.revision[..16])
    }

    fn token_at(&self, span: Span) -> Result<usize, Problem> {
        self.tokens
            .iter()
            .position(|t| t.span.line == span.line && t.span.col == span.col && !structural(t))
            .ok_or_else(|| Problem::new("source-range", "source token was not found"))
    }

    fn header_end(&self, span: Span) -> Result<usize, Problem> {
        let start = self.token_at(span)?;
        let end = self.tokens[start..]
            .iter()
            .take_while(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
            .filter(|t| !structural(t))
            .last()
            .ok_or_else(|| Problem::new("source-range", "node header was not found"))?;
        Ok(self.offset(end.span, true))
    }

    fn attribute(&self, a: &Attr) -> Result<Option<Attribute>, Problem> {
        let at = self.token_at(a.span)?;
        let written =
            &self.text[self.offset(a.span, false)..self.offset(self.tokens[at].span, true)];
        let name = a.name.rsplit(' ').next().unwrap_or(&a.name);
        if written != name && !name.starts_with(&format!("{written}.")) {
            return Ok(None);
        }
        let bare = name == "autofocus"
            && !matches!(
                self.tokens.get(at + 1).map(|t| &t.kind),
                Some(TokenKind::Punct("="))
            );
        let equal = if bare {
            at
        } else {
            (at + 1..(at + 5).min(self.tokens.len()))
                .find(|&i| matches!(self.tokens[i].kind, TokenKind::Punct("=")))
                .ok_or_else(|| {
                    Problem::new("source-range", "attribute has no written assignment")
                })?
        };
        let limit = self.tokens[equal..]
            .iter()
            .position(|t| matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
            .map_or(self.tokens.len(), |i| equal + i + 1);
        let end = if bare {
            self.tokens[at].span
        } else {
            contract_syntax::parser::attribute_value_end(&self.tokens[equal - 1..limit], 0)
                .map_err(|e| Problem::new("source-range", e.message))?
        };
        let value_start = self.offset(self.tokens[if bare { at } else { equal + 1 }].span, false);
        let value_end = self.offset(end, true);
        Ok(Some(Attribute {
            name: a.name.clone(),
            start: self.offset(a.span, false),
            value_start,
            end: value_end,
            handle: self.handle("a", self.offset(a.span, false)),
            bare,
        }))
    }

    fn nodes(&self) -> Result<Vec<AuthoredNode>, Problem> {
        let mut nodes = Vec::new();
        for c in &self.file.components {
            self.walk(&c.view, &c.name, None, &mut nodes)?;
        }
        Ok(nodes)
    }

    fn walk(
        &self,
        nodes: &[Node],
        component: &str,
        parent: Option<&str>,
        out: &mut Vec<AuthoredNode>,
    ) -> Result<(), Problem> {
        for node in nodes {
            let start = self.offset(node.span(), false);
            let end = self.header_end(node.span())?;
            let handle = self.handle("n", start);
            if parent == Some(handle.as_str()) {
                continue;
            }
            let (kind, tag, attrs) = match node {
                Node::Element { tag, attrs, .. } => ("element", tag.as_str(), attrs.as_slice()),
                Node::Use { name, args, .. } => ("component", name.as_str(), args.as_slice()),
                Node::Children { .. } => ("children", "children", &[][..]),
                Node::When { .. } => ("if", "if", &[][..]),
                Node::Each { .. } => ("for", "for", &[][..]),
                Node::Match { .. } => ("match", "match", &[][..]),
            };
            let mut attributes = Vec::new();
            for a in attrs.iter().filter(|a| a.written) {
                if let Some(a) = self.attribute(a)? {
                    attributes.push(a);
                }
            }
            let text = match node {
                Node::Element { tag, .. } if tag == "text" => Some(node),
                Node::Element { tag, children, .. } if tag == "button" => {
                    let mut labels = children.iter().filter(
                        |child| matches!(child, Node::Element { tag, .. } if tag == "text"),
                    );
                    labels.next().filter(|_| labels.next().is_none())
                }
                _ => None,
            };
            let literal = match text {
                Some(Node::Element { positional, .. }) => match positional.as_slice() {
                    [contract_syntax::Expr::Str(_, span)] => {
                        let token = &self.tokens[self.token_at(*span)?];
                        matches!(token.kind, TokenKind::Str(_)).then(|| {
                            (
                                self.offset(token.span, false),
                                self.offset(token.span, true),
                            )
                        })
                    }
                    _ => None,
                },
                _ => None,
            };
            out.push(AuthoredNode {
                component: component.into(),
                parent: parent.map(str::to_string),
                handle: handle.clone(),
                kind,
                tag: tag.into(),
                start,
                end,
                attributes,
                literal,
            });
            match node {
                Node::Element { children, .. } | Node::Use { children, .. } => {
                    self.walk(children, component, Some(&handle), out)?;
                }
                Node::When {
                    then, otherwise, ..
                } => {
                    self.walk(then, component, Some(&handle), out)?;
                    self.walk(otherwise, component, Some(&handle), out)?;
                }
                Node::Each { body, .. } => self.walk(body, component, Some(&handle), out)?,
                Node::Match { some, none, .. } => {
                    self.walk(&some.1, component, Some(&handle), out)?;
                    self.walk(none, component, Some(&handle), out)?;
                }
                Node::Children { .. } => {}
            }
        }
        Ok(())
    }
}

fn structural(token: &Token) -> bool {
    matches!(
        token.kind,
        TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
    )
}

struct AuthoredNode {
    component: String,
    handle: String,
    parent: Option<String>,
    kind: &'static str,
    tag: String,
    start: usize,
    end: usize,
    attributes: Vec<Attribute>,
    literal: Option<(usize, usize)>,
}

struct Attribute {
    name: String,
    handle: String,
    start: usize,
    value_start: usize,
    end: usize,
    bare: bool,
}

fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n, _) => n.clone(),
        TypeExpr::Option(t, _) => format!("option<{}>", type_name(t)),
        TypeExpr::List(t, _) => format!("{}[]", type_name(t)),
        TypeExpr::Enum(names, _) => format!(
            "enum<{}>",
            names
                .iter()
                .map(|n| json!(n).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn component_json(source: &Source, c: &Component) -> Value {
    let params = |params: &[contract_syntax::Param]| {
        params
            .iter()
            .map(|p| json!({"name":p.name,"type":p.ty.as_ref().map(type_name)}))
            .collect::<Vec<_>>()
    };
    json!({
        "name":c.name,"handle":source.handle("c",source.offset(c.span,false)),
        "props":params(&c.props),"injects":params(&c.injects),
        "states":c.states.iter().map(|s| &s.name).collect::<Vec<_>>(),
        "derives":c.derives.iter().map(|s| &s.name).collect::<Vec<_>>(),
        "actions":c.actions.iter().map(|a| json!({"name":a.name,"params":params(&a.params)})).collect::<Vec<_>>()
    })
}

fn component_context(c: &Component) -> Value {
    let parameters = |kind: &str, params: &[contract_syntax::Param]| {
        params.iter().map(|p|json!({"kind":kind,"name":p.name,"type":p.ty.as_ref().map(type_name),"line":p.span.line})).collect::<Vec<_>>()
    };
    let groups = [
        parameters("prop",&c.props),
        parameters("inject",&c.injects),
        c.states.iter().map(|s|json!({"kind":"state","name":s.name,"type":match &s.expr {
            contract_syntax::Expr::Typed(_, ty, _) => Some(type_name(ty)),
            _ => None,
        },"line":s.span.line})).collect(),
        c.derives.iter().map(|s|json!({"kind":"derive","name":s.name,"type":null,"line":s.span.line})).collect(),
        c.resources.iter().map(|r|json!({"kind":"resource","name":r.name,"type":type_name(&r.shape),"perItem":r.each.is_some(),"line":r.span.line})).collect(),
        c.mutations.iter().map(|m|json!({"kind":"mutation","name":m.name,"type":format!("option<{}>",type_name(&m.shape)),"line":m.span.line})).collect(),
        c.actions.iter().map(|a|json!({"kind":"action","name":a.name,"type":"action","line":a.span.line,
            "params":a.params.iter().map(|p|json!({"name":p.name,"type":p.ty.as_ref().map(type_name)})).collect::<Vec<_>>()
        })).collect(),
    ];
    let total: usize = groups.iter().map(Vec::len).sum();
    let mut members = Vec::new();
    let mut index = 0;
    while members.len() < 32 {
        let before = members.len();
        for group in &groups {
            if let Some(member) = group.get(index) {
                members.push(member.clone());
            }
            if members.len() == 32 {
                break;
            }
        }
        if members.len() == before {
            break;
        }
        index += 1;
    }
    members.sort_by_key(|member| member["line"].as_u64().unwrap_or_default());
    let mut omitted = BTreeMap::new();
    for group in &groups {
        if let Some(first) = group.first() {
            let kind = first["kind"].as_str().unwrap();
            let count = members
                .iter()
                .filter(|member| member["kind"] == kind)
                .count();
            if group.len() > count {
                omitted.insert(kind, group.len() - count);
            }
        }
    }
    json!({"component":c.name,"members":members,"total":total,"more":total-members.len(),"omittedByKind":omitted})
}

fn inspect_source(
    path: &Path,
    component: Option<&str>,
    tag: Option<&str>,
    needles: &[&str],
    detailed: bool,
    filters: &Filters,
    context: bool,
) -> Result<Value, Problem> {
    let source = Source::read(path)?;
    if component.is_some_and(|name| !source.file.components.iter().any(|c| c.name == name)) {
        return Err(Problem::new(
            "inspect-component",
            "component is not declared in this source file",
        ));
    }
    let nodes =
        if component.is_some() || !needles.is_empty() || tag.is_some() || !filters.is_empty() {
            source.nodes()?
        } else {
            Vec::new()
        };
    let selected = query_nodes(&source, &nodes, component, tag, needles, filters);
    let mut result = if !detailed {
        let components = source
            .file
            .components
            .iter()
            .filter(|c| component.is_none_or(|name| c.name == name))
            .map(|c| {
                let count = Node::walk(&c.view)
                    .map(|n| source.offset(n.span(), false))
                    .collect::<BTreeSet<_>>()
                    .len();
                json!({"name":c.name,"nodes":count})
            })
            .collect::<Vec<_>>();
        json!({"ok":true,"file":source.path,"revision":source.revision,"components":components,
            "nodes":selected.iter().take(20).map(|n|compact_node(&source,n)).collect::<Vec<_>>(),
            "total":selected.len(),"more":selected.len().saturating_sub(20)})
    } else {
        let components = source
            .file
            .components
            .iter()
            .filter(|c| component.is_none_or(|name| c.name == name))
            .map(|c| component_json(&source, c))
            .collect::<Vec<_>>();
        let nodes = selected.iter()
            .map(|n| json!({"handle":n.handle,"kind":n.kind,"tag":n.tag,"component":n.component,"parent":n.parent,
                "range":{"start":n.start,"end":n.end},"snippet":&source.text[n.start..n.end],
                "attributes":n.attributes.iter().map(|a|json!({"name":a.name,"handle":a.handle,"value":if a.bare {"true"} else {&source.text[a.value_start..a.end]},"range":{"start":a.value_start,"end":a.end},"implicit":a.bare})).collect::<Vec<_>>()
            })).collect::<Vec<_>>();
        json!({"version":1,"ok":true,"file":source.path,"revision":source.revision,"components":components,"nodes":nodes})
    };
    if context {
        let owners: Vec<_> = source
            .file
            .components
            .iter()
            .filter(|c| selected.iter().any(|n| n.component == c.name))
            .collect();
        result["context"] = json!(owners
            .iter()
            .take(4)
            .map(|c| component_context(c))
            .collect::<Vec<_>>());
        result["contextMore"] = json!(owners.len().saturating_sub(4));
    }
    Ok(result)
}

fn query_nodes<'a>(
    source: &Source,
    nodes: &'a [AuthoredNode],
    component: Option<&str>,
    tag: Option<&str>,
    needles: &[&str],
    filters: &Filters,
) -> Vec<&'a AuthoredNode> {
    let descendants = if tag.is_some() && !needles.is_empty() {
        let parents: BTreeMap<_, _> = nodes
            .iter()
            .map(|n| (n.handle.as_str(), n.parent.as_deref()))
            .collect();
        let mut matched = BTreeSet::new();
        for node in nodes.iter().filter(|n| {
            component.is_none_or(|c| n.component == c)
                && needles.iter().any(|text| matches_node(source, n, text))
        }) {
            let mut at = Some(node.handle.as_str());
            while let Some(handle) = at {
                if !matched.insert(handle) {
                    break;
                }
                at = parents.get(handle).copied().flatten();
            }
        }
        Some(matched)
    } else {
        None
    };
    nodes
        .iter()
        .filter(|n| component.is_none_or(|c| n.component == c) && tag.is_none_or(|t| n.tag == t))
        .filter(|n| {
            descendants.as_ref().map_or_else(
                || needles.is_empty() || needles.iter().any(|text| matches_node(source, n, text)),
                |found| found.contains(n.handle.as_str()),
            )
        })
        .filter(|n| filters.matches(source, n))
        .collect()
}

fn matches_node(source: &Source, node: &AuthoredNode, needle: &str) -> bool {
    source.text[node.start..node.end].contains(needle)
        || node.tag.contains(needle)
        || node
            .attributes
            .iter()
            .any(|a| a.name.contains(needle) || source.text[a.start..a.end].contains(needle))
}

fn compact_node(source: &Source, node: &AuthoredNode) -> Value {
    json!({"component":node.component,"line":source.starts.partition_point(|start|*start<=node.start),
        "handle":node.handle,"source":&source.text[node.start..node.end]})
}

fn direct_edit(
    path: &Path,
    component: Option<&str>,
    tag: Option<&str>,
    groups: &[Selection],
    apply: bool,
    filters: &Filters,
) -> Result<Value, Problem> {
    let source = writable_source(path)?;
    let nodes = source.nodes()?;
    let mut edits = Vec::new();
    for group in groups {
        let matches: Vec<_> = query_nodes(
            &source,
            &nodes,
            component,
            tag,
            &[group.needle.as_str()],
            filters,
        )
        .into_iter()
        .filter(|n| n.kind == "element")
        .collect();
        if matches.len() != 1 {
            return Err(Problem {
                id: "edit-selection",
                message: format!(
                    "--match {:?} selected {} elements; exactly one is required",
                    group.needle,
                    matches.len()
                ),
                diagnostics: Vec::new(),
                candidates: matches
                    .iter()
                    .take(20)
                    .map(|n| compact_node(&source, n))
                    .collect(),
            });
        }
        for operation in &group.edits {
            let mut operation = operation.clone();
            operation["target"] = json!(matches[0].handle);
            edits.push(operation);
        }
    }
    let request = json!({"revision":source.revision,"edits":edits});
    edit_snapshot(source, &request, apply)
}

fn compact_receipt(receipt: Value, diagnostics: bool) -> Value {
    let warnings = receipt["diagnostics"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut codes: BTreeMap<&str, usize> = BTreeMap::new();
    for warning in warnings {
        *codes
            .entry(warning["id"].as_str().unwrap_or("unknown"))
            .or_default() += 1;
    }
    let mut compact = json!({"ok":receipt["ok"],"applied":receipt["applied"],"validated":true,
        "revision":receipt["revision"],"newRevision":receipt["newRevision"],
        "changes":receipt["changes"].as_array().map_or(0,Vec::len),
        "warnings":{"count":warnings.len(),"codes":codes},"diff":receipt["diff"]});
    if diagnostics {
        compact["diagnostics"] = receipt["diagnostics"].clone();
    }
    compact
}

fn format_receipt(mut receipt: Value, detailed: bool, diagnostics: bool) -> Value {
    let compact = compact_receipt(receipt.clone(), diagnostics);
    if !detailed {
        return compact;
    }
    receipt["validated"] = json!(true);
    receipt["warnings"] = compact["warnings"].clone();
    if !diagnostics {
        receipt.as_object_mut().unwrap().remove("diagnostics");
    }
    receipt
}

struct Change {
    start: usize,
    end: usize,
    text: String,
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, Problem> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Problem::new("edit-request", format!("`{key}` must be a string")))
}

fn expression(edit: &Value) -> Result<String, Problem> {
    let value = required(edit, "value")?;
    let wrapped = format!("value={value}");
    let tokens =
        Lexer::tokenize(&wrapped, 1).map_err(|e| Problem::new("edit-expression", e.message))?;
    let end = contract_syntax::parser::attribute_value_end(&tokens, 0)
        .map_err(|e| Problem::new("edit-expression", e.message))?;
    let last = tokens
        .iter()
        .rfind(|t| !structural(t))
        .ok_or_else(|| Problem::new("edit-expression", "an expression is required"))?;
    if last.span != end {
        return Err(Problem::new(
            "edit-expression",
            "value must contain exactly one expression",
        ));
    }
    // Comments and trailing newlines can otherwise hide or detach the next original token.
    let starts: Vec<_> = std::iter::once(0)
        .chain(wrapped.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let offset = starts[end.line as usize - 1] + end.end_col as usize - 1;
    if !wrapped[offset..].is_empty() {
        return Err(Problem::new(
            "edit-expression",
            "value must end at its last expression token",
        ));
    }
    Ok(value.to_string())
}

fn edit_source(path: &Path, request: &Value, apply: bool) -> Result<Value, Problem> {
    edit_snapshot(writable_source(path)?, request, apply)
}

fn writable_source(path: &Path) -> Result<Source, Problem> {
    if path.components().any(|c| c.as_os_str() == "node_modules") {
        return Err(Problem::new(
            "edit-package",
            "package sources are read-only",
        ));
    }
    let source = Source::read(path)?;
    if source
        .path
        .components()
        .any(|c| c.as_os_str() == "node_modules")
    {
        return Err(Problem::new(
            "edit-package",
            "package sources are read-only",
        ));
    }
    Ok(source)
}

fn edit_snapshot(source: Source, request: &Value, apply: bool) -> Result<Value, Problem> {
    if required(request, "revision")? != source.revision {
        return Err(Problem::new(
            "edit-stale",
            "source changed since inspection; inspect again",
        ));
    }
    let edits = request
        .get("edits")
        .and_then(Value::as_array)
        .filter(|edits| !edits.is_empty())
        .ok_or_else(|| Problem::new("edit-request", "`edits` must be a non-empty array"))?;
    let nodes = source.nodes()?;
    let mut changes = Vec::new();
    let mut touched = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for edit in edits {
        let op = required(edit, "op")?;
        let target = required(edit, "target")?;
        let (node, attribute) = nodes
            .iter()
            .find_map(|n| {
                if n.handle == target {
                    Some((n, None))
                } else {
                    n.attributes
                        .iter()
                        .find(|a| a.handle == target)
                        .map(|a| (n, Some(a)))
                }
            })
            .ok_or_else(|| {
                Problem::new(
                    "edit-target",
                    "target is not in this source snapshot; inspect again",
                )
            })?;
        if request
            .get("component")
            .and_then(Value::as_str)
            .is_some_and(|c| c != node.component)
        {
            return Err(Problem::new(
                "edit-target",
                "target belongs to another component",
            ));
        }
        selected.insert(node.handle.as_str());
        let change = match op {
            "set-text" => {
                let (start, end) =
                    node.literal
                        .filter(|_| attribute.is_none())
                        .ok_or_else(|| {
                            Problem::new(
                            "edit-text",
                            "text edits require one existing literal text or direct button label",
                        )
                        })?;
                let mut text = String::from("\"");
                for ch in required(edit, "value")?.chars() {
                    match ch {
                        '"' => text.push_str("\\\""),
                        '\\' => text.push_str("\\\\"),
                        '\n' => text.push_str("\\n"),
                        '\t' => text.push_str("\\t"),
                        ch if ch.is_control() => text.push_str(&format!("\\u{{{:x}}}", ch as u32)),
                        ch => text.push(ch),
                    }
                }
                text.push('"');
                Change { start, end, text }
            }
            "replace-expr" => {
                let a = attribute.ok_or_else(|| {
                    Problem::new("edit-target", "replace-expr requires an attribute handle")
                })?;
                let value = expression(edit)?;
                Change {
                    start: a.value_start,
                    end: a.end,
                    text: if a.bare {
                        format!("{}={value}", a.name)
                    } else {
                        value
                    },
                }
            }
            "set-attr" | "remove-attr" | "or-attr" | "and-attr" => {
                if attribute.is_some() || node.kind != "element" {
                    return Err(Problem::new(
                        "edit-target",
                        "attribute edits require an element handle",
                    ));
                }
                let name = required(edit, "name")?;
                if !touched.insert((target.to_string(), name.to_string())) {
                    return Err(Problem::new(
                        "edit-conflict",
                        "the batch edits an attribute more than once",
                    ));
                }
                let found = node
                    .attributes
                    .iter()
                    .filter(|a| a.name == name)
                    .collect::<Vec<_>>();
                if found.len() > 1 {
                    return Err(Problem::new(
                        "edit-target",
                        "attribute is repeated; use its exact handle with replace-expr",
                    ));
                }
                match (op, found.first()) {
                    ("set-attr" | "or-attr" | "and-attr", Some(a)) => {
                        let mut value = expression(edit)?;
                        if op != "set-attr" {
                            let original = if a.bare {
                                "true"
                            } else {
                                &source.text[a.value_start..a.end]
                            };
                            let operator = if op == "or-attr" { "or" } else { "and" };
                            value = format!("({original}) {operator} ({value})");
                        }
                        Change {
                            start: a.value_start,
                            end: a.end,
                            text: if a.bare {
                                format!("{}={value}", a.name)
                            } else {
                                value
                            },
                        }
                    }
                    ("set-attr", None) => {
                        let valid = Lexer::tokenize(&format!("{name}=0"),1).is_ok_and(|tokens| matches!(&tokens[..], [Token{kind:TokenKind::Ident(n),..}, Token{kind:TokenKind::Punct("="),..}, ..] if n == name));
                        if !valid || name.contains(char::is_whitespace) {
                            return Err(Problem::new(
                                "edit-name",
                                "name must be one Contract attribute identifier",
                            ));
                        }
                        Change {
                            start: node.end,
                            end: node.end,
                            text: format!(" {name}={}", expression(edit)?),
                        }
                    }
                    ("remove-attr", Some(a)) => {
                        let mut start = a.start;
                        while start > 0 && source.text.as_bytes()[start - 1] == b' ' {
                            start -= 1;
                        }
                        Change {
                            start,
                            end: a.end,
                            text: String::new(),
                        }
                    }
                    _ => {
                        return Err(Problem::new(
                            "edit-target",
                            "attribute is not written on this node",
                        ));
                    }
                }
            }
            _ => {
                return Err(Problem::new(
                    "edit-op",
                    format!("unsupported operation `{op}`"),
                ));
            }
        };
        changes.push(change);
    }
    changes.sort_by_key(|c| (c.start, c.end));
    let mut merged: Vec<Change> = Vec::new();
    for c in changes {
        if let Some(previous) = merged.last_mut().filter(|previous| {
            previous.start == previous.end && c.start == c.end && previous.start == c.start
        }) {
            previous.text.push_str(&c.text);
        } else {
            merged.push(c);
        }
    }
    let changes = merged;
    for pair in changes.windows(2) {
        if pair[1].start < pair[0].end || pair[0].start == pair[1].start {
            return Err(Problem::new(
                "edit-conflict",
                "batch edits overlap; combine them or inspect between edits",
            ));
        }
    }
    let mut after = source.text.clone();
    for c in changes.iter().rev() {
        after.replace_range(c.start..c.end, &c.text);
    }
    let (validation, warnings) =
        contract::with_contract_texts(vec![(source.path.clone(), after.clone())], || {
            let contract::Advised {
                compiled: (compiled, mut warnings),
                surfaces,
                clocks,
            } = contract::compile_path_advised(&source.path, false, contract_lower::Profile::Web);
            let mut refused = match &surfaces {
                Ok(s) if s.newer.is_none() => s.findings.clone(),
                Ok(_) => Vec::new(),
                Err(all) => all
                    .iter()
                    .filter(|e| e.id == "analyze-surface-declaration")
                    .cloned()
                    .collect(),
            };
            refused.extend(crate::ambient::check(&source.path));
            let stale = surfaces.as_ref().ok().filter(|s| s.newer.is_some());
            warnings.extend(
                clocks
                    .into_iter()
                    .chain(stale.map(|s| s.findings.clone()).unwrap_or_default())
                    .map(|e| contract::CompileError {
                        pass: "warning",
                        ..e
                    }),
            );
            let compiled = match compiled {
                Ok(_) if !refused.is_empty() => Err(refused),
                Ok(compiled) => Ok(compiled),
                Err(mut errors) => {
                    errors.extend(refused);
                    if !errors
                        .iter()
                        .any(|e| contract_syntax::migrate::is_retired(&e.id))
                    {
                        errors.truncate(contract::MAX_DIAGNOSTICS);
                    }
                    Err(errors)
                }
            };
            (compiled, warnings)
        });
    if let Err(errors) = validation {
        return Err(Problem {
            id: "edit-compile",
            message: "proposed source was refused; no files were written".into(),
            diagnostics: errors
                .iter()
                .chain(&warnings)
                .map(|e| {
                    serde_json::from_str(&e.to_json())
                        .unwrap_or_else(|_| json!({"message":e.to_string()}))
                })
                .collect(),
            candidates: nodes
                .iter()
                .filter(|node| selected.contains(node.handle.as_str()))
                .take(20)
                .map(|node| compact_node(&source, node))
                .collect(),
        });
    }
    if apply {
        write(&source, &after)?;
    }
    Ok(
        json!({"ok":true,"applied":apply,"revision":source.revision,"newRevision":digest(&after),
            "changes":changes.iter().map(|c|json!({"start":c.start,"end":c.end,"before":&source.text[c.start..c.end],"after":c.text})).collect::<Vec<_>>(),"diagnostics":warnings.iter().map(|e|serde_json::from_str::<Value>(&e.to_json()).unwrap_or_else(|_|json!({"message":e.to_string()}))).collect::<Vec<_>>(),
            "diff":crate::diff::unified(source.path.file_name().unwrap_or_default().to_string_lossy().as_ref(),&source.text,&after)
        }),
    )
}

fn write(source: &Source, text: &str) -> Result<(), Problem> {
    let current = std::fs::read_to_string(&source.path)
        .map_err(|e| Problem::new("source-read", e.to_string()))?;
    if current != source.text {
        return Err(Problem::new(
            "edit-stale",
            "source changed during validation; inspect again",
        ));
    }
    let temporary = source.path.with_file_name(format!(
        ".contract-edit-{}-{}.tmp",
        std::process::id(),
        &source.revision[..16]
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&temporary)
        .map_err(|e| Problem::new("edit-write", e.to_string()))?;
    let result = (|| {
        file.set_permissions(std::fs::metadata(&source.path)?.permissions())?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if std::fs::read_to_string(&source.path)? != source.text {
            return Err(std::io::Error::other(
                "source changed before replacement; inspect again",
            ));
        }
        std::fs::rename(&temporary, &source.path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(Problem::new("edit-write", e.to_string()));
    }
    Ok(())
}
