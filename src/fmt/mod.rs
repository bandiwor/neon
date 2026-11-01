//! The source formatter behind `neon fmt` and the language server's formatting request.
//!
//! The formatter never moves, adds or removes a token. It keeps the author's line breaks and adds
//! the ones the layout asks for: one statement per line, a function body on its own lines (a short
//! `if`/`else` expression may stay on one line), a blank line after every function, and a break
//! for every line wider than `MAX_WIDTH` (before the loosest operator, in a method chain, inside the
//! last bracket pair, or after `=`). It normalizes indentation (driven by brackets, with one extra
//! level for continuation lines), the spacing between tokens, runs of blank lines, trailing
//! whitespace and the final newline. Operators are classified with the parser's tree, so `a - b`
//! and `-b`, or `a < b` and `Vec<T>`, are told apart. Comments are kept verbatim.
//!
//! Before returning, the output is lexed again and compared with the input token by token (and
//! comment by comment); a mismatch is reported as an error instead of being written.

use std::fmt;

use crate::diag::{DiagBag, FileId};
use crate::syntax::{
    parse_source, tokenize, BinOp, Interner, Keyword, LitPool, NodeId, NodeKind, Token, TokenKind, UnOp,
};

/// Spaces per indentation level.
const INDENT: usize = 4;

/// Lines wider than this are broken.
const MAX_WIDTH: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

/// Formats a whole module. Source with syntax errors is rejected (it has no reliable structure).
pub fn format_source(src: &str) -> Result<String, FormatError> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src).replace("\r\n", "\n");
    let out = format_normalized(&src)?;
    verify(&src, &out)?;
    Ok(out)
}

fn lex(src: &str) -> Result<Vec<Token>, FormatError> {
    let mut interner = Interner::new();
    let mut pool = LitPool::new();
    let mut diags = DiagBag::new();
    let mut tokens = tokenize(FileId(0), src, &mut interner, &mut pool, &mut diags);
    if let Some(d) = diags.iter().next().filter(|_| diags.has_errors()) {
        return Err(FormatError(format!("cannot format: {}", d.message)));
    }
    tokens.pop(); // Eof
    Ok(collapse_format_strings(tokens))
}

/// A format string is lexed into many tokens (text, fields, the expressions inside); the formatter
/// keeps it verbatim, so it sees one token spanning the whole string.
fn collapse_format_strings(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut depth = 0usize;
    let mut start = None;
    for t in tokens {
        match t.kind {
            TokenKind::FStrBegin => {
                if depth == 0 {
                    start = Some(t);
                }
                depth += 1;
            }
            TokenKind::FStrEnd => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(first) = start.take() {
                        out.push(Token { kind: TokenKind::FStrBegin, span: first.span.to(t.span) });
                    }
                }
            }
            _ if depth > 0 => {}
            _ => out.push(t),
        }
    }
    out
}

fn text<'a>(src: &'a str, t: &Token) -> &'a str {
    &src[t.span.lo as usize..t.span.hi as usize]
}

/// A token, a comment, or the end of a line.
enum Piece {
    Tok(usize),
    Line(String),
    Block(String),
}

struct Line {
    pieces: Vec<Piece>,
    blank_before: bool,
}

/// Splits `gap` (whitespace and comments between two tokens) and appends to `lines`.
struct Builder {
    lines: Vec<Line>,
    cur: Vec<Piece>,
    blank_before: bool,
    newlines: usize,
}

impl Builder {
    fn push(&mut self, piece: Piece) {
        if self.cur.is_empty() {
            self.blank_before = !self.lines.is_empty() && self.newlines >= 2;
        }
        self.newlines = 0;
        self.cur.push(piece);
    }

    fn newline(&mut self) {
        if !self.cur.is_empty() {
            let pieces = std::mem::take(&mut self.cur);
            self.lines.push(Line { pieces, blank_before: self.blank_before });
        }
        self.newlines += 1;
    }

    fn gap(&mut self, gap: &str) {
        let b = gap.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\n' => {
                    self.newline();
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    let end = gap[i..].find('\n').map_or(gap.len(), |n| i + n);
                    self.push(Piece::Line(gap[i..end].trim_end().to_string()));
                    i = end;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    let mut depth = 0usize;
                    let mut j = i;
                    while j < b.len() {
                        if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                            depth += 1;
                            j += 2;
                        } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                            depth -= 1;
                            j += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            j += 1;
                        }
                    }
                    self.push(Piece::Block(gap[i..j].to_string()));
                    i = j;
                }
                _ => i += 1, // spaces, tabs, stray carriage returns
            }
        }
    }
}

fn is_closer(k: TokenKind) -> bool {
    matches!(k, TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket)
}

fn is_opener(k: TokenKind) -> bool {
    matches!(k, TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket)
}

fn bin_token(op: BinOp) -> TokenKind {
    use TokenKind as T;
    match op {
        BinOp::Add => T::Plus,
        BinOp::Sub => T::Minus,
        BinOp::Mul => T::Star,
        BinOp::Div => T::Slash,
        BinOp::Rem => T::Percent,
        BinOp::BitAnd => T::Amp,
        BinOp::BitOr => T::Pipe,
        BinOp::BitXor => T::Caret,
        BinOp::Shl => T::Shl,
        BinOp::Shr => T::Shr,
        BinOp::And => T::AmpAmp,
        BinOp::Or => T::PipePipe,
        BinOp::Eq => T::EqEq,
        BinOp::Ne => T::BangEq,
        BinOp::Lt => T::Lt,
        BinOp::Le => T::Le,
        BinOp::Gt => T::Gt,
        BinOp::Ge => T::Ge,
    }
}

/// Operators that are always binary or always separated by spaces.
fn is_spaced_kind(k: TokenKind) -> bool {
    use TokenKind as T;
    matches!(
        k,
        T::Arrow
            | T::FatArrow
            | T::Eq
            | T::EqEq
            | T::BangEq
            | T::Le
            | T::Ge
            | T::PlusEq
            | T::MinusEq
            | T::StarEq
            | T::SlashEq
            | T::PercentEq
            | T::AmpEq
            | T::PipeEq
            | T::CaretEq
            | T::ShlEq
            | T::ShrEq
    )
}

fn is_word(k: TokenKind) -> bool {
    use TokenKind as T;
    matches!(
        k,
        T::Ident(_) | T::Kw(_) | T::Int(_) | T::Float(_) | T::Str(_) | T::FStrBegin | T::Char(_) | T::Underscore
    )
}

/// Keywords that behave like values (`self`, `true`, `i32`), unlike `if`, `return` or `as`.
fn is_value_kw(k: Keyword) -> bool {
    k.is_primitive_type() || matches!(k, Keyword::SelfValue | Keyword::SelfType | Keyword::True | Keyword::False)
}

struct Ctx<'a> {
    toks: &'a [Token],
    binary: Vec<bool>,
    unary: Vec<bool>,
}

impl Ctx<'_> {
    /// Whether a space separates the adjacent tokens `a` and `b` (indices) on one line.
    fn space(&self, a: usize, b: usize) -> bool {
        use TokenKind as T;
        let (ka, kb) = (self.toks[a].kind, self.toks[b].kind);
        let kept = self.toks[a].span.hi != self.toks[b].span.lo;
        // never a space
        if matches!(kb, T::Comma | T::Semi | T::Colon | T::Question | T::Dot | T::ColonColon | T::RParen | T::RBracket)
            || matches!(ka, T::Dot | T::ColonColon | T::LParen | T::LBracket)
            || self.unary[a]
        {
            return false;
        }
        // always a space
        if matches!(ka, T::Colon) || is_spaced_kind(ka) || is_spaced_kind(kb) || self.binary[a] || self.binary[b] {
            return true;
        }
        match (ka, kb) {
            (T::Comma, T::RBrace) => kept,
            (T::Comma, _) => true,
            (T::Semi, T::RBrace) => kept,
            (T::Semi, _) => true,
            (T::LBrace, T::RBrace) => false,
            (T::LBrace, _) | (_, T::RBrace) => true,
            (T::RBrace, T::Kw(Keyword::Else | Keyword::Elif)) => true,
            (_, T::LBrace) => !matches!(ka, T::LParen | T::LBracket),
            (T::Kw(Keyword::Fn), T::LParen) | (T::Kw(Keyword::Impl), T::Lt) => false,
            (_, T::LParen | T::LBracket) => match ka {
                T::Ident(_) | T::RParen | T::RBracket | T::Question | T::Gt => false,
                T::Kw(k) if is_value_kw(k) => false,
                T::Kw(_) => true,
                _ => kept,
            },
            (T::Kw(k), _) if !is_value_kw(k) => true,
            _ if is_word(ka) && is_word(kb) => true,
            _ => kept,
        }
    }
}

/// Where the formatter itself breaks lines (as opposed to keeping the author's breaks).
struct Layout {
    /// A new line must start before this token.
    nl_before: Vec<bool>,
    /// The closing brace of a function body: a blank line follows.
    fn_end: Vec<bool>,
    /// The matching bracket of an opener or closer (`usize::MAX` if unmatched).
    pair: Vec<usize>,
    /// The first tokens of the children of a brace group (statements, arms, fields, items).
    groups: std::collections::HashMap<usize, Vec<usize>>,
}

fn layout(ast: &crate::syntax::Ast, toks: &[Token]) -> Layout {
    use std::collections::HashMap;
    let n = toks.len();
    let first_at = |off: u32| toks.partition_point(|t| t.span.lo < off);

    // brackets
    let mut pair = vec![usize::MAX; n];
    let mut parent = vec![usize::MAX; n];
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        parent[i] = stack.last().copied().unwrap_or(usize::MAX);
        if is_opener(t.kind) {
            stack.push(i);
        } else if is_closer(t.kind) {
            if let Some(o) = stack.pop() {
                pair[o] = i;
                pair[i] = o;
            }
        }
    }

    let brace_of = |child: NodeId| -> Option<usize> {
        let f = first_at(ast.span(child).lo);
        (f < n).then_some(parent[f]).filter(|&o| o != usize::MAX && toks[o].kind == TokenKind::LBrace)
    };
    let open_of_block = |b: NodeId| -> Option<usize> {
        let o = first_at(ast.span(b).lo);
        (o < n && toks[o].kind == TokenKind::LBrace && pair[o] != usize::MAX).then_some(o)
    };
    let start_of = |c: NodeId| first_at(ast.span(c).lo);

    let mut expand = vec![false; n];
    let mut fn_end = vec![false; n];
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut else_ifs: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    let mut module_starts: Vec<usize> = Vec::new();
    let group = |open: Option<usize>, children: &[NodeId], groups: &mut HashMap<usize, Vec<usize>>| {
        if let Some(o) = open {
            groups.entry(o).or_default().extend(children.iter().map(|&c| start_of(c)));
        }
    };

    for (id, node) in ast.iter() {
        match node.kind {
            NodeKind::Module { items } => module_starts = ast.list(items).iter().map(|&c| start_of(c)).collect(),
            NodeKind::Block { stmts, tail } => {
                let mut children: Vec<NodeId> = ast.list(stmts).to_vec();
                children.extend(tail.get());
                if let Some(o) = open_of_block(id) {
                    expand[o] |= children.len() >= 2;
                    group(Some(o), &children, &mut groups);
                }
            }
            NodeKind::FnDecl { body, .. } => {
                if let Some(b) = body.get() {
                    if let Some(o) = open_of_block(b) {
                        fn_end[pair[o]] = true;
                        if let NodeKind::Block { stmts, tail } = *ast.kind(b) {
                            expand[o] |= !stmts.is_empty() || tail.get().is_some();
                        }
                    }
                }
            }
            NodeKind::If { else_branch, .. } => {
                if let Some(e) = else_branch.get() {
                    if matches!(ast.kind(e), NodeKind::If { .. }) {
                        else_ifs.insert(e);
                    }
                }
            }
            NodeKind::Match { arms, .. } => {
                if let Some(&f) = ast.list(arms).first() {
                    group(brace_of(f), ast.list(arms), &mut groups);
                }
            }
            NodeKind::StructDecl { fields, .. } => {
                if let Some(&f) = ast.list(fields).first() {
                    group(brace_of(f), ast.list(fields), &mut groups);
                }
            }
            NodeKind::EnumDecl { variants, .. } => {
                if let Some(&f) = ast.list(variants).first() {
                    group(brace_of(f), ast.list(variants), &mut groups);
                }
            }
            NodeKind::TraitDecl { items, .. } | NodeKind::ImplDecl { items, .. } => {
                if let Some(&f) = ast.list(items).first() {
                    group(brace_of(f), ast.list(items), &mut groups);
                }
            }
            NodeKind::StructLit { fields, .. } => {
                if let Some(&f) = ast.list(fields).first() {
                    group(brace_of(f), ast.list(fields), &mut groups);
                }
            }
            NodeKind::MapLit { entries } => {
                if let Some(&f) = ast.list(entries).first() {
                    group(brace_of(f), ast.list(entries), &mut groups);
                }
            }
            NodeKind::SetLit { elems } => {
                if let Some(&f) = ast.list(elems).first() {
                    group(brace_of(f), ast.list(elems), &mut groups);
                }
            }
            _ => {}
        }
    }
    // the branches of an if / elif / else chain are laid out alike
    for (id, node) in ast.iter() {
        if let NodeKind::If { .. } = node.kind {
            if else_ifs.contains(&id) {
                continue;
            }
            let mut chain = Vec::new();
            let mut cur = Some(id);
            while let Some(c) = cur.take() {
                let NodeKind::If { then_branch, else_branch, .. } = *ast.kind(c) else { break };
                chain.extend(open_of_block(then_branch));
                match else_branch.get() {
                    Some(e) if matches!(ast.kind(e), NodeKind::If { .. }) => cur = Some(e),
                    Some(e) => chain.extend(open_of_block(e)),
                    None => {}
                }
            }
            chains.push(chain);
        }
    }
    // a brace that holds an expanded brace is expanded too, and so is every branch next to it
    loop {
        let mut changed = false;
        for o in 0..n {
            if !expand[o] {
                continue;
            }
            let mut p = parent[o];
            while p != usize::MAX {
                if toks[p].kind == TokenKind::LBrace && !expand[p] {
                    expand[p] = true;
                    changed = true;
                }
                p = parent[p];
            }
        }
        for chain in &chains {
            if chain.iter().any(|&o| expand[o]) {
                for &o in chain {
                    if !expand[o] {
                        expand[o] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut nl_before = vec![false; n];
    for o in 0..n {
        if !expand[o] {
            continue;
        }
        let c = pair[o];
        if c > o + 1 {
            nl_before[o + 1] = true;
            nl_before[c] = true;
            for &s in groups.get(&o).into_iter().flatten() {
                if s < n {
                    nl_before[s] = true;
                }
            }
        }
    }
    for &s in module_starts.iter().skip(1) {
        if s < n {
            nl_before[s] = true;
        }
    }
    Layout { nl_before, fn_end, pair, groups }
}

/// Binding strength of a binary operator token (lower binds looser); the line breaks before the loosest.
fn binary_class(k: TokenKind) -> u8 {
    use TokenKind as T;
    match k {
        T::PipePipe => 0,
        T::AmpAmp => 1,
        T::EqEq | T::BangEq | T::Lt | T::Le | T::Gt | T::Ge => 2,
        T::Pipe => 3,
        T::Caret => 4,
        T::Amp => 5,
        T::Shl | T::Shr => 6,
        T::Plus | T::Minus => 7,
        _ => 8,
    }
}

fn is_assign_like(k: TokenKind) -> bool {
    is_spaced_kind(k)
        && !matches!(k, TokenKind::EqEq | TokenKind::BangEq | TokenKind::Le | TokenKind::Ge | TokenKind::Arrow)
}

/// The text of a run of pieces: tokens spaced by the rules above, comments verbatim.
fn render(src: &str, toks: &[Token], cx: &Ctx<'_>, pieces: &[Piece]) -> String {
    let mut text = String::new();
    let mut prev_tok: Option<usize> = None;
    let mut first_piece = true;
    for p in pieces {
        match p {
            Piece::Tok(i) => {
                if let Some(a) = prev_tok {
                    if cx.space(a, *i) {
                        text.push(' ');
                    }
                } else if !first_piece {
                    text.push(' '); // after a block comment
                }
                text.push_str(self::text(src, &toks[*i]));
                prev_tok = Some(*i);
            }
            Piece::Block(c) | Piece::Line(c) => {
                if !first_piece {
                    text.push(' ');
                }
                text.push_str(c);
                prev_tok = None;
            }
        }
        first_piece = false;
    }
    text
}

/// Breaks an over-long line: before the loosest binary operators or the dots of a method chain, else
/// inside the last bracket pair that opens and closes on the line, else after an `=`. `None` when
/// there is nothing to break.
fn split_line(line: &Line, toks: &[Token], cx: &Ctx<'_>, lay: &Layout) -> Option<Vec<Line>> {
    // (piece index, token index) of the tokens, and their depth relative to the line
    let at: Vec<(usize, usize)> = line
        .pieces
        .iter()
        .enumerate()
        .filter_map(|(pi, p)| if let Piece::Tok(i) = p { Some((pi, *i)) } else { None })
        .collect();
    if at.len() < 2 {
        return None;
    }
    let mut depth = 0i32;
    let mut ds = Vec::with_capacity(at.len());
    for &(_, ti) in &at {
        let k = toks[ti].kind;
        if is_closer(k) {
            depth -= 1;
        }
        ds.push(depth);
        if is_opener(k) {
            depth += 1;
        }
    }
    let base = ds.iter().copied().min().unwrap_or(0);
    let top = |j: usize| ds[j] == base;
    let kind = |j: usize| toks[at[j].1].kind;

    // breaks[j]: a new line starts at the j-th token
    let mut breaks: Vec<usize> = Vec::new();

    // binary operators: before each of the loosest
    let ops: Vec<usize> = (1..at.len()).filter(|&j| top(j) && cx.binary[at[j].1]).collect();
    if let Some(class) = ops.iter().map(|&j| binary_class(kind(j))).min() {
        breaks = ops.into_iter().filter(|&j| binary_class(kind(j)) == class).collect();
    }
    // method chains
    if breaks.is_empty() {
        let dots: Vec<usize> = (1..at.len()).filter(|&j| top(j) && kind(j) == TokenKind::Dot).collect();
        if dots.len() >= 2 {
            breaks = dots;
        }
    }
    // where the pair opened by the j-th token is broken open: after the opener, between the items, before the closer
    let pos_of = |ti: usize| at.iter().position(|&(_, t)| t == ti);
    let pair_in_line = |j: usize| -> Option<usize> {
        if !(top(j) && is_opener(kind(j))) {
            return None;
        }
        pos_of(lay.pair[at[j].1]).filter(|&c| c > j + 1)
    };
    let open_up = |j: usize, c: usize| -> Vec<usize> {
        let open = at[j].1;
        let mut starts: Vec<usize> = vec![j + 1];
        match lay.groups.get(&open) {
            Some(children) if kind(j) == TokenKind::LBrace => {
                starts.extend((j + 1..c).filter(|&m| children.contains(&at[m].1)));
            }
            _ => {
                let inner = ds[j] + 1;
                starts.extend(
                    (j + 1..c).filter(|&m| kind(m) == TokenKind::Comma && ds[m] == inner && m + 1 < c).map(|m| m + 1),
                );
            }
        }
        starts.push(c);
        starts
    };
    // a block after a broken condition is opened up as well
    if !breaks.is_empty() {
        for j in 0..at.len() {
            if kind(j) == TokenKind::LBrace {
                if let Some(c) = pair_in_line(j) {
                    breaks.extend(open_up(j, c));
                }
            }
        }
    }
    // the last bracket pair that opens and closes on the line
    if breaks.is_empty() {
        if let Some((j, c)) = (0..at.len()).rev().find_map(|j| pair_in_line(j).map(|c| (j, c))) {
            breaks = open_up(j, c);
        }
    }
    breaks.sort_unstable();
    breaks.dedup();
    // after an assignment or a match arrow
    if breaks.is_empty() {
        if let Some(j) =
            (0..at.len() - 1).find(|&j| top(j) && (is_assign_like(kind(j)) || kind(j) == TokenKind::FatArrow))
        {
            breaks = vec![j + 1];
        }
    }
    if breaks.is_empty() {
        return None;
    }

    let mut cuts: Vec<usize> = breaks.iter().map(|&j| at[j].0).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut parts: Vec<Line> = Vec::new();
    let mut start = 0usize;
    let mut pieces = line.pieces.iter();
    let mut taken = 0usize;
    cuts.push(line.pieces.len());
    for cut in cuts {
        if cut <= start {
            continue;
        }
        let chunk: Vec<Piece> = pieces.by_ref().take(cut - taken).map(clone_piece).collect();
        taken = cut;
        parts.push(Line { pieces: chunk, blank_before: parts.is_empty() && line.blank_before });
        start = cut;
    }
    (parts.len() > 1).then_some(parts)
}

fn clone_piece(p: &Piece) -> Piece {
    match p {
        Piece::Tok(i) => Piece::Tok(*i),
        Piece::Line(s) => Piece::Line(s.clone()),
        Piece::Block(s) => Piece::Block(s.clone()),
    }
}

fn format_normalized(src: &str) -> Result<String, FormatError> {
    // syntax errors make the structure unreliable
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let ast = parse_source(FileId(0), src, &mut interner, &mut diags);
    if let Some(d) = diags.iter().next().filter(|_| diags.has_errors()) {
        return Err(FormatError(format!("cannot format: {}", d.message)));
    }
    let toks = lex(src)?;

    // classify operators with the tree
    let first_at = |off: u32| toks.partition_point(|t| t.span.lo < off);
    let mut binary = vec![false; toks.len()];
    let mut unary = vec![false; toks.len()];
    for (_, node) in ast.iter() {
        match node.kind {
            NodeKind::Binary { op, lhs, .. } => {
                let mut i = first_at(ast.span(lhs).hi);
                while i < toks.len() && toks[i].kind == TokenKind::RParen {
                    i += 1;
                }
                if i < toks.len() && toks[i].kind == bin_token(op) {
                    binary[i] = true;
                }
            }
            NodeKind::Unary { op, .. } => {
                let i = first_at(node.span.lo);
                let want = match op {
                    UnOp::Neg => TokenKind::Minus,
                    UnOp::Not => TokenKind::Bang,
                    UnOp::BitNot => TokenKind::Tilde,
                };
                if i < toks.len() && toks[i].kind == want {
                    unary[i] = true;
                }
            }
            _ => {}
        }
    }
    let cx = Ctx { toks: &toks, binary, unary };
    let lay = layout(&ast, &toks);

    // lines of pieces: the author's breaks, plus the ones the layout asks for
    let mut b = Builder { lines: Vec::new(), cur: Vec::new(), blank_before: false, newlines: 0 };
    let mut prev_hi = 0usize;
    for (i, t) in toks.iter().enumerate() {
        b.gap(&src[prev_hi..t.span.lo as usize]);
        if lay.nl_before[i] && !b.cur.is_empty() {
            b.newline();
        }
        b.push(Piece::Tok(i));
        prev_hi = t.span.hi as usize;
    }
    b.gap(&src[prev_hi..]);
    b.newline();
    let mut queue: std::collections::VecDeque<Line> = b.lines.into();

    // indentation
    struct Open {
        line_indent: usize,
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut out = String::new();
    let mut prev_code_last: Option<usize> = None;
    let mut prev_emitted_opener = false;
    let mut prev_line_last: Option<usize> = None;
    while let Some(line) = queue.pop_front() {
        let toks_in_line: Vec<usize> =
            line.pieces.iter().filter_map(|p| if let Piece::Tok(i) = p { Some(*i) } else { None }).collect();
        let first = toks_in_line.first().copied();
        let leading_closers = toks_in_line.iter().take_while(|&&i| is_closer(toks[i].kind)).count();
        // a line that only has comments sits at the inner level
        let cont = match first {
            None => false,
            Some(f) => {
                leading_closers == 0
                    && (toks[f].kind == TokenKind::Dot
                        || cx.binary[f]
                        || prev_code_last.is_some_and(|p| {
                            let k = toks[p].kind;
                            let assigns = is_spaced_kind(k) && !matches!(k, TokenKind::EqEq | TokenKind::BangEq);
                            cx.binary[p] || assigns
                        }))
            }
        };
        let indent = if leading_closers > 0 {
            stack.last().map_or(0, |e| e.line_indent)
        } else {
            stack.last().map_or(0, |e| e.line_indent + 1) + usize::from(cont)
        };

        let text = render(src, &toks, &cx, &line.pieces);
        // too wide (not counting a trailing line comment): break it and look at the parts instead
        let code_width = match line.pieces.last() {
            Some(Piece::Line(c)) => text.chars().count() - c.chars().count(),
            _ => text.chars().count(),
        };
        if indent * INDENT + code_width > MAX_WIDTH {
            if let Some(parts) = split_line(&line, &toks, &cx, &lay) {
                for part in parts.into_iter().rev() {
                    queue.push_front(part);
                }
                continue;
            }
        }

        // blank lines: at most one; none after an opener, before a closer, or at the start; one after a function
        let starts_with_closer = leading_closers > 0;
        let after_fn = prev_line_last.is_some_and(|i| lay.fn_end[i]) && !toks_in_line.is_empty();
        if (line.blank_before || after_fn) && !out.is_empty() && !prev_emitted_opener && !starts_with_closer {
            out.push('\n');
        }
        for _ in 0..indent * INDENT {
            out.push(' ');
        }
        out.push_str(&text);
        out.push('\n');

        // update bracket state with this line's tokens
        for &i in &toks_in_line {
            let k = toks[i].kind;
            if is_opener(k) {
                // a block opened at the end of a continuation line is indented like the statement
                let ends_line = toks_in_line.last() == Some(&i);
                let block = k == TokenKind::LBrace && ends_line && cont && leading_closers == 0;
                stack.push(Open { line_indent: indent - usize::from(block) });
            } else if is_closer(k) {
                stack.pop();
            }
        }
        if let Some(&last) = toks_in_line.last() {
            prev_code_last = Some(last);
        }
        prev_line_last = toks_in_line.last().copied();
        prev_emitted_opener = toks_in_line.last().is_some_and(|&i| is_opener(toks[i].kind));
    }
    Ok(out)
}

/// Comments of `src`, in order, with trailing whitespace of line comments removed.
fn comments(src: &str, toks: &[Token]) -> Vec<String> {
    let mut b = Builder { lines: Vec::new(), cur: Vec::new(), blank_before: false, newlines: 0 };
    let mut prev_hi = 0usize;
    for t in toks {
        b.gap(&src[prev_hi..t.span.lo as usize]);
        prev_hi = t.span.hi as usize;
    }
    b.gap(&src[prev_hi..]);
    b.newline();
    b.lines
        .into_iter()
        .flat_map(|l| l.pieces)
        .filter_map(|p| match p {
            Piece::Line(c) | Piece::Block(c) => Some(c),
            Piece::Tok(_) => None,
        })
        .collect()
}

/// The formatted text must contain exactly the same tokens and comments.
fn verify(src: &str, out: &str) -> Result<(), FormatError> {
    let (a, b) = (lex(src)?, lex(out)?);
    let same_tokens =
        a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.kind == y.kind && text(src, x) == text(out, y));
    if !same_tokens || comments(src, &a) != comments(out, &b) {
        return Err(FormatError("internal error: the formatted text differs from the input".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
