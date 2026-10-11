//! Syntax colouring: one small lexer driven by a per-language description.

use std::path::Path;

/// What a stretch of text is, for colouring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
}

/// A run of one token kind, from the end of the previous span up to byte `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub end: usize,
    pub token: Token,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// No colouring.
    Text,
    /// Keywords, strings, numbers and comments.
    Code,
    /// Tags and attributes.
    Markup,
    Markdown,
}

#[derive(Debug)]
pub struct Language {
    pub name: &'static str,
    family: Family,
    extensions: &'static [&'static str],
    /// Whole file names, for files without a telling extension.
    files: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static [u8],
    /// Quotes whose strings may run over several lines.
    multiline: &'static [u8],
    /// Three quotes open a string that ends at the next three.
    triple_quotes: bool,
    /// `'a` is a lifetime and `name!` a macro call.
    rust: bool,
    /// `/…/` can be a regular expression.
    regex_literals: bool,
    /// The lexer tells strings well enough to report unbalanced brackets.
    brackets: bool,
    keywords_ignore_case: bool,
    keywords: &'static [&'static str],
}

const BASE: Language = Language {
    name: "Plain Text",
    family: Family::Code,
    extensions: &[],
    files: &[],
    line_comments: &[],
    block_comment: None,
    quotes: b"\"'",
    multiline: &[],
    triple_quotes: false,
    rust: false,
    regex_literals: false,
    brackets: false,
    keywords_ignore_case: false,
    keywords: &[],
};

const C_COMMENTS: Option<(&str, &str)> = Some(("/*", "*/"));

static PLAIN: Language = Language {
    family: Family::Text,
    extensions: &["txt", "log"],
    ..BASE
};

static LANGUAGES: &[Language] = &[
    Language {
        name: "Rust",
        brackets: true,
        extensions: &["rs"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        multiline: b"\"",
        rust: true,
        keywords: &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        ..BASE
    },
    Language {
        name: "JavaScript",
        brackets: true,
        regex_literals: true,
        extensions: &["js", "mjs", "cjs", "jsx"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        quotes: b"\"'`",
        multiline: b"`",
        keywords: JS_KEYWORDS,
        ..BASE
    },
    Language {
        name: "TypeScript",
        brackets: true,
        regex_literals: true,
        extensions: &["ts", "mts", "cts", "tsx"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        quotes: b"\"'`",
        multiline: b"`",
        keywords: JS_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Python",
        brackets: true,
        extensions: &["py", "pyw", "pyi"],
        line_comments: &["#"],
        triple_quotes: true,
        keywords: &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "self", "True", "try", "while", "with", "yield",
        ],
        ..BASE
    },
    Language {
        name: "C",
        brackets: true,
        extensions: &["c", "h"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: C_KEYWORDS,
        ..BASE
    },
    Language {
        name: "C++",
        brackets: true,
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: C_KEYWORDS,
        ..BASE
    },
    Language {
        name: "C#",
        extensions: &["cs"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Java",
        brackets: true,
        extensions: &["java"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Kotlin",
        extensions: &["kt", "kts"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        triple_quotes: true,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Go",
        brackets: true,
        extensions: &["go"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        quotes: b"\"'`",
        multiline: b"`",
        keywords: &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "false",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "nil",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "true",
            "type",
            "var",
        ],
        ..BASE
    },
    Language {
        name: "Swift",
        extensions: &["swift"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        triple_quotes: true,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "PHP",
        extensions: &["php"],
        line_comments: &["//", "#"],
        block_comment: C_COMMENTS,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Ruby",
        extensions: &["rb"],
        files: &["Gemfile", "Rakefile"],
        line_comments: &["#"],
        keywords: &[
            "alias", "and", "begin", "break", "case", "class", "def", "do", "else", "elsif", "end",
            "ensure", "false", "for", "if", "in", "module", "next", "nil", "not", "or", "require",
            "rescue", "return", "self", "super", "then", "true", "unless", "until", "when",
            "while", "yield",
        ],
        ..BASE
    },
    Language {
        name: "Lua",
        extensions: &["lua"],
        line_comments: &["--"],
        keywords: &[
            "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
            "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        ],
        ..BASE
    },
    Language {
        name: "Shell",
        extensions: &["sh", "bash", "zsh", "fish"],
        files: &["Makefile", ".gitignore", ".env"],
        line_comments: &["#"],
        keywords: &[
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if",
            "in", "local", "return", "then", "until", "while",
        ],
        ..BASE
    },
    Language {
        name: "PowerShell",
        extensions: &["ps1", "psm1"],
        line_comments: &["#"],
        block_comment: Some(("<#", "#>")),
        keywords_ignore_case: true,
        keywords: &[
            "break", "catch", "continue", "do", "else", "elseif", "finally", "for", "foreach",
            "function", "if", "in", "param", "return", "switch", "throw", "try", "while",
        ],
        ..BASE
    },
    Language {
        name: "SQL",
        extensions: &["sql", "pls", "plsql", "tsql", "psql"],
        line_comments: &["--"],
        block_comment: C_COMMENTS,
        keywords_ignore_case: true,
        keywords: &[
            "add",
            "alter",
            "and",
            "as",
            "asc",
            "begin",
            "by",
            "case",
            "commit",
            "constraint",
            "create",
            "default",
            "delete",
            "desc",
            "distinct",
            "drop",
            "else",
            "end",
            "exists",
            "foreign",
            "from",
            "group",
            "having",
            "if",
            "in",
            "index",
            "inner",
            "insert",
            "into",
            "is",
            "join",
            "key",
            "left",
            "like",
            "limit",
            "not",
            "null",
            "on",
            "or",
            "order",
            "outer",
            "primary",
            "references",
            "right",
            "select",
            "set",
            "table",
            "then",
            "union",
            "unique",
            "update",
            "values",
            "view",
            "when",
            "where",
        ],
        ..BASE
    },
    Language {
        name: "JSON",
        brackets: true,
        extensions: &["json", "jsonc", "webmanifest"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: &["true", "false", "null"],
        ..BASE
    },
    Language {
        name: "TOML",
        extensions: &["toml", "ini", "cfg", "conf"],
        files: &["Cargo.lock"],
        line_comments: &["#", ";"],
        triple_quotes: true,
        keywords: &["true", "false"],
        ..BASE
    },
    Language {
        name: "YAML",
        extensions: &["yml", "yaml"],
        line_comments: &["#"],
        keywords: &["true", "false", "null", "yes", "no"],
        ..BASE
    },
    Language {
        name: "CSS",
        extensions: &["css", "scss", "sass", "less"],
        block_comment: C_COMMENTS,
        keywords: &["important", "import", "media", "from", "to"],
        ..BASE
    },
    Language {
        name: "HTML",
        family: Family::Markup,
        extensions: &["html", "htm", "vue", "svelte"],
        // Not coloured as keywords: offered as completions.
        keywords: HTML_TAGS,
        ..BASE
    },
    Language {
        name: "XML",
        family: Family::Markup,
        extensions: &["xml", "svg", "xaml", "csproj", "plist"],
        ..BASE
    },
    Language {
        name: "Markdown",
        family: Family::Markdown,
        extensions: &["md", "markdown"],
        ..BASE
    },
    Language {
        name: "HTTP",
        extensions: &["http", "rest"],
        line_comments: &["#", "//"],
        keywords: &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"],
        ..BASE
    },
    Language {
        name: "Batch",
        extensions: &["bat", "cmd"],
        line_comments: &["::", "REM ", "rem "],
        keywords_ignore_case: true,
        keywords: &[
            "call", "do", "echo", "else", "exit", "for", "goto", "if", "in", "not", "set",
        ],
        ..BASE
    },
    Language {
        name: "Dockerfile",
        extensions: &["dockerfile"],
        files: &["Dockerfile", "Containerfile"],
        line_comments: &["#"],
        keywords: &[
            "FROM",
            "RUN",
            "CMD",
            "COPY",
            "ADD",
            "ENV",
            "ARG",
            "EXPOSE",
            "WORKDIR",
            "ENTRYPOINT",
            "USER",
            "VOLUME",
            "LABEL",
            "AS",
            "HEALTHCHECK",
            "SHELL",
            "ONBUILD",
            "STOPSIGNAL",
        ],
        ..BASE
    },
    Language {
        name: "Dart",
        brackets: true,
        extensions: &["dart"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        triple_quotes: true,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Scala",
        extensions: &["scala", "sc", "sbt"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        triple_quotes: true,
        keywords: &[
            "abstract",
            "case",
            "catch",
            "class",
            "def",
            "do",
            "else",
            "extends",
            "false",
            "final",
            "finally",
            "for",
            "given",
            "if",
            "implicit",
            "import",
            "lazy",
            "match",
            "new",
            "null",
            "object",
            "override",
            "package",
            "private",
            "protected",
            "return",
            "sealed",
            "super",
            "this",
            "throw",
            "trait",
            "true",
            "try",
            "type",
            "using",
            "val",
            "var",
            "while",
            "with",
            "yield",
        ],
        ..BASE
    },
    Language {
        name: "Groovy",
        extensions: &["groovy", "gradle", "gvy"],
        files: &["Jenkinsfile"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        triple_quotes: true,
        keywords: JAVA_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Objective-C",
        brackets: true,
        extensions: &["mm"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: C_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Zig",
        brackets: true,
        extensions: &["zig", "zon"],
        line_comments: &["//"],
        keywords: &[
            "and",
            "break",
            "catch",
            "comptime",
            "const",
            "continue",
            "defer",
            "else",
            "enum",
            "errdefer",
            "error",
            "export",
            "extern",
            "false",
            "fn",
            "for",
            "if",
            "inline",
            "null",
            "or",
            "orelse",
            "pub",
            "return",
            "struct",
            "switch",
            "test",
            "true",
            "try",
            "undefined",
            "union",
            "unreachable",
            "var",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "D",
        brackets: true,
        extensions: &["d", "di"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: C_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Nim",
        extensions: &["nim", "nims", "nimble"],
        line_comments: &["#"],
        triple_quotes: true,
        keywords: &[
            "and", "case", "const", "discard", "echo", "elif", "else", "false", "for", "from",
            "func", "if", "import", "in", "include", "let", "nil", "not", "object", "of", "or",
            "proc", "return", "true", "type", "var", "when", "while",
        ],
        ..BASE
    },
    Language {
        name: "Haskell",
        extensions: &["hs", "lhs", "cabal"],
        line_comments: &["--"],
        block_comment: Some(("{-", "-}")),
        quotes: b"\"",
        keywords: &[
            "as",
            "case",
            "class",
            "data",
            "deriving",
            "do",
            "else",
            "if",
            "import",
            "in",
            "infixl",
            "infixr",
            "instance",
            "let",
            "module",
            "newtype",
            "of",
            "qualified",
            "then",
            "type",
            "where",
        ],
        ..BASE
    },
    Language {
        name: "OCaml",
        extensions: &["ml", "mli"],
        block_comment: Some(("(*", "*)")),
        quotes: b"\"",
        keywords: &[
            "and",
            "begin",
            "else",
            "end",
            "exception",
            "false",
            "for",
            "fun",
            "function",
            "if",
            "in",
            "let",
            "match",
            "module",
            "mutable",
            "of",
            "open",
            "rec",
            "sig",
            "struct",
            "then",
            "true",
            "try",
            "type",
            "val",
            "when",
            "while",
            "with",
        ],
        ..BASE
    },
    Language {
        name: "F#",
        extensions: &["fs", "fsx", "fsi"],
        line_comments: &["//"],
        block_comment: Some(("(*", "*)")),
        quotes: b"\"",
        triple_quotes: true,
        keywords: &[
            "async",
            "do",
            "elif",
            "else",
            "false",
            "for",
            "fun",
            "if",
            "in",
            "let",
            "match",
            "member",
            "module",
            "mutable",
            "namespace",
            "of",
            "open",
            "rec",
            "return",
            "then",
            "true",
            "try",
            "type",
            "use",
            "when",
            "while",
            "with",
            "yield",
        ],
        ..BASE
    },
    Language {
        name: "Elixir",
        extensions: &["ex", "exs", "heex"],
        line_comments: &["#"],
        triple_quotes: true,
        keywords: &[
            "alias",
            "and",
            "case",
            "cond",
            "def",
            "defimpl",
            "defmacro",
            "defmodule",
            "defp",
            "defprotocol",
            "defstruct",
            "do",
            "else",
            "end",
            "false",
            "fn",
            "if",
            "import",
            "in",
            "nil",
            "not",
            "or",
            "raise",
            "require",
            "rescue",
            "true",
            "try",
            "unless",
            "use",
            "when",
            "with",
        ],
        ..BASE
    },
    Language {
        name: "Erlang",
        extensions: &["erl", "hrl"],
        line_comments: &["%"],
        quotes: b"\"",
        keywords: &[
            "after", "and", "begin", "case", "catch", "end", "fun", "if", "not", "of", "or",
            "receive", "try", "when",
        ],
        ..BASE
    },
    Language {
        name: "Perl",
        extensions: &["pl", "pm", "t"],
        line_comments: &["#"],
        keywords: &[
            "die", "else", "elsif", "for", "foreach", "if", "last", "local", "my", "next", "our",
            "package", "print", "require", "return", "sub", "unless", "until", "use", "while",
        ],
        ..BASE
    },
    Language {
        name: "R",
        extensions: &["r", "rmd"],
        line_comments: &["#"],
        keywords: &[
            "FALSE", "NA", "NULL", "TRUE", "break", "else", "for", "function", "if", "in",
            "library", "next", "repeat", "return", "while",
        ],
        ..BASE
    },
    Language {
        name: "Julia",
        extensions: &["jl"],
        line_comments: &["#"],
        block_comment: Some(("#=", "=#")),
        quotes: b"\"",
        triple_quotes: true,
        keywords: &[
            "abstract", "begin", "break", "catch", "const", "continue", "do", "else", "elseif",
            "end", "export", "false", "finally", "for", "function", "if", "import", "in", "let",
            "macro", "module", "mutable", "nothing", "return", "struct", "true", "try", "using",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "Assembly",
        extensions: &["asm", "s", "nasm"],
        line_comments: &[";"],
        keywords_ignore_case: true,
        keywords: &[
            "add", "and", "call", "cmp", "db", "dd", "dq", "dw", "extern", "global", "jmp", "je",
            "jne", "lea", "mov", "or", "pop", "push", "ret", "section", "sub", "xor",
        ],
        ..BASE
    },
    Language {
        name: "Ada",
        extensions: &["adb", "ads"],
        line_comments: &["--"],
        quotes: b"\"",
        keywords_ignore_case: true,
        keywords: &[
            "begin",
            "body",
            "case",
            "constant",
            "else",
            "elsif",
            "end",
            "for",
            "function",
            "if",
            "in",
            "is",
            "loop",
            "out",
            "package",
            "procedure",
            "record",
            "return",
            "then",
            "type",
            "use",
            "when",
            "while",
            "with",
        ],
        ..BASE
    },
    Language {
        name: "Fortran",
        extensions: &["f90", "f95", "f03", "f08", "f", "for"],
        line_comments: &["!"],
        keywords_ignore_case: true,
        keywords: &[
            "call",
            "contains",
            "do",
            "else",
            "end",
            "function",
            "if",
            "implicit",
            "integer",
            "module",
            "none",
            "program",
            "real",
            "return",
            "subroutine",
            "then",
            "use",
        ],
        ..BASE
    },
    Language {
        name: "COBOL",
        extensions: &["cob", "cbl", "cpy"],
        line_comments: &["*>"],
        keywords_ignore_case: true,
        keywords: &[
            "data",
            "display",
            "division",
            "else",
            "identification",
            "if",
            "move",
            "perform",
            "pic",
            "procedure",
            "run",
            "section",
            "stop",
            "to",
        ],
        ..BASE
    },
    Language {
        name: "Pascal",
        extensions: &["pas", "pp", "dpr", "lpr"],
        line_comments: &["//"],
        block_comment: Some(("{", "}")),
        keywords_ignore_case: true,
        keywords: &[
            "begin",
            "class",
            "const",
            "do",
            "else",
            "end",
            "for",
            "function",
            "if",
            "procedure",
            "program",
            "record",
            "repeat",
            "then",
            "type",
            "unit",
            "until",
            "uses",
            "var",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "Tcl",
        extensions: &["tcl"],
        line_comments: &["#"],
        quotes: b"\"",
        keywords: &[
            "else", "elseif", "expr", "for", "foreach", "if", "proc", "puts", "return", "set",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "Awk",
        extensions: &["awk"],
        line_comments: &["#"],
        quotes: b"\"",
        keywords: &[
            "BEGIN", "END", "else", "for", "function", "if", "next", "print", "printf", "return",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "GraphQL",
        extensions: &["graphql", "gql"],
        line_comments: &["#"],
        triple_quotes: true,
        keywords: &[
            "directive",
            "enum",
            "extend",
            "false",
            "fragment",
            "implements",
            "input",
            "interface",
            "mutation",
            "null",
            "on",
            "query",
            "scalar",
            "schema",
            "subscription",
            "true",
            "type",
            "union",
        ],
        ..BASE
    },
    Language {
        name: "Protocol Buffers",
        extensions: &["proto"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: &[
            "enum", "import", "map", "message", "oneof", "option", "optional", "package",
            "repeated", "required", "reserved", "returns", "rpc", "service", "stream", "syntax",
        ],
        ..BASE
    },
    Language {
        name: "Nix",
        extensions: &["nix"],
        line_comments: &["#"],
        block_comment: C_COMMENTS,
        quotes: b"\"",
        keywords: &[
            "assert", "else", "false", "if", "import", "in", "inherit", "let", "null", "rec",
            "then", "true", "with",
        ],
        ..BASE
    },
    Language {
        name: "HCL",
        extensions: &["tf", "hcl", "tfvars"],
        line_comments: &["#", "//"],
        block_comment: C_COMMENTS,
        quotes: b"\"",
        keywords: &[
            "data",
            "false",
            "for",
            "if",
            "in",
            "locals",
            "module",
            "null",
            "output",
            "provider",
            "resource",
            "terraform",
            "true",
            "variable",
        ],
        ..BASE
    },
    Language {
        name: "LaTeX",
        extensions: &["tex", "sty", "cls", "bib"],
        line_comments: &["%"],
        quotes: b"",
        ..BASE
    },
    Language {
        name: "MATLAB",
        extensions: &["m"],
        line_comments: &["%"],
        keywords: &[
            "break",
            "case",
            "catch",
            "classdef",
            "continue",
            "else",
            "elseif",
            "end",
            "false",
            "for",
            "function",
            "if",
            "methods",
            "otherwise",
            "properties",
            "return",
            "switch",
            "true",
            "try",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "Solidity",
        brackets: true,
        extensions: &["sol"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: &[
            "address",
            "bool",
            "calldata",
            "contract",
            "else",
            "emit",
            "enum",
            "event",
            "external",
            "false",
            "for",
            "function",
            "if",
            "import",
            "interface",
            "internal",
            "is",
            "library",
            "mapping",
            "memory",
            "modifier",
            "payable",
            "pragma",
            "private",
            "public",
            "pure",
            "require",
            "return",
            "returns",
            "storage",
            "struct",
            "true",
            "uint",
            "view",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "Vyper",
        extensions: &["vy"],
        line_comments: &["#"],
        triple_quotes: true,
        keywords: &[
            "False", "True", "assert", "def", "elif", "else", "event", "for", "from", "if",
            "import", "in", "pass", "return", "struct",
        ],
        ..BASE
    },
    Language {
        name: "Move",
        brackets: true,
        extensions: &["move"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        quotes: b"\"",
        keywords: &[
            "abort", "const", "else", "false", "fun", "has", "if", "let", "loop", "module", "mut",
            "public", "return", "struct", "true", "use", "while",
        ],
        ..BASE
    },
    Language {
        name: "GDScript",
        extensions: &["gd"],
        line_comments: &["#"],
        triple_quotes: true,
        keywords: &[
            "and",
            "class_name",
            "const",
            "elif",
            "else",
            "export",
            "extends",
            "false",
            "for",
            "func",
            "if",
            "in",
            "match",
            "not",
            "null",
            "onready",
            "or",
            "pass",
            "return",
            "signal",
            "true",
            "var",
            "while",
        ],
        ..BASE
    },
    Language {
        name: "GLSL",
        brackets: true,
        extensions: &["glsl", "vert", "frag", "comp", "geom"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: SHADER_KEYWORDS,
        ..BASE
    },
    Language {
        name: "HLSL",
        brackets: true,
        extensions: &["hlsl", "fx", "shader", "cginc"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: SHADER_KEYWORDS,
        ..BASE
    },
    Language {
        name: "WGSL",
        brackets: true,
        extensions: &["wgsl"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        keywords: SHADER_KEYWORDS,
        ..BASE
    },
    Language {
        name: "Verilog",
        extensions: &["v", "sv", "svh", "vh"],
        line_comments: &["//"],
        block_comment: C_COMMENTS,
        quotes: b"\"",
        keywords: &[
            "always",
            "assign",
            "begin",
            "case",
            "else",
            "end",
            "endcase",
            "endfunction",
            "endmodule",
            "for",
            "function",
            "generate",
            "if",
            "initial",
            "inout",
            "input",
            "localparam",
            "logic",
            "module",
            "negedge",
            "output",
            "parameter",
            "posedge",
            "reg",
            "wire",
        ],
        ..BASE
    },
    Language {
        name: "VHDL",
        extensions: &["vhd", "vhdl"],
        line_comments: &["--"],
        quotes: b"\"",
        keywords_ignore_case: true,
        keywords: &[
            "architecture",
            "begin",
            "case",
            "component",
            "else",
            "elsif",
            "end",
            "entity",
            "generic",
            "if",
            "in",
            "is",
            "library",
            "of",
            "out",
            "port",
            "process",
            "signal",
            "then",
            "use",
            "when",
        ],
        ..BASE
    },
    Language {
        name: "Wolfram",
        extensions: &["wl", "wls"],
        block_comment: Some(("(*", "*)")),
        quotes: b"\"",
        ..BASE
    },
    Language {
        name: "Cypher",
        extensions: &["cypher", "cql"],
        line_comments: &["//"],
        keywords_ignore_case: true,
        keywords: &[
            "and", "as", "by", "create", "delete", "limit", "match", "merge", "not", "optional",
            "or", "order", "return", "set", "unwind", "where", "with",
        ],
        ..BASE
    },
    Language {
        name: "Datalog",
        extensions: &["dl"],
        line_comments: &["%"],
        quotes: b"\"",
        ..BASE
    },
    Language {
        name: "WebAssembly",
        extensions: &["wat", "wast"],
        line_comments: &[";;"],
        quotes: b"\"",
        keywords: &[
            "call", "export", "func", "global", "import", "local", "memory", "module", "param",
            "result", "table",
        ],
        ..BASE
    },
    Language {
        name: "Lisp",
        extensions: &["lisp", "el", "clj", "cljs", "scm", "rkt"],
        line_comments: &[";"],
        quotes: b"\"",
        keywords: &[
            "cond", "def", "defmacro", "defn", "defun", "fn", "if", "lambda", "let", "quote",
        ],
        ..BASE
    },
    Language {
        name: "CMake",
        extensions: &["cmake"],
        files: &["CMakeLists.txt"],
        line_comments: &["#"],
        quotes: b"\"",
        keywords_ignore_case: true,
        keywords: &[
            "else",
            "elseif",
            "endforeach",
            "endfunction",
            "endif",
            "foreach",
            "function",
            "if",
            "include",
            "project",
            "set",
        ],
        ..BASE
    },
    Language {
        name: "Astro",
        family: Family::Markup,
        extensions: &["astro"],
        keywords: HTML_TAGS,
        ..BASE
    },
    Language {
        name: "MDX",
        family: Family::Markdown,
        extensions: &["mdx"],
        ..BASE
    },
    Language {
        name: "CSV",
        family: Family::Text,
        extensions: &["csv", "tsv"],
        ..BASE
    },
    Language {
        name: "reStructuredText",
        family: Family::Text,
        extensions: &["rst"],
        ..BASE
    },
    Language {
        name: "AsciiDoc",
        family: Family::Text,
        extensions: &["adoc", "asciidoc"],
        ..BASE
    },
];

/// Shared by the shading languages (GLSL, HLSL, WGSL).
const SHADER_KEYWORDS: &[&str] = &[
    "bool", "break", "const", "continue", "discard", "else", "false", "float", "fn", "for", "if",
    "in", "inout", "int", "let", "mat3", "mat4", "out", "return", "struct", "true", "uniform",
    "var", "vec2", "vec3", "vec4", "void", "while",
];

const HTML_TAGS: &[&str] = &[
    "article", "aside", "body", "button", "canvas", "details", "dialog", "div", "fieldset",
    "figure", "footer", "form", "head", "header", "html", "iframe", "img", "input", "label",
    "link", "main", "meta", "nav", "option", "script", "section", "select", "span", "strong",
    "style", "summary", "table", "tbody", "textarea", "thead", "title", "video",
];

const JS_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
];

const C_KEYWORDS: &[&str] = &[
    "auto",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "constexpr",
    "continue",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "false",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "namespace",
    "new",
    "nullptr",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

/// Shared by the C-family object languages (Java, C#, Kotlin, Swift, PHP).
const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "boolean",
    "break",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "foreach",
    "fun",
    "func",
    "function",
    "guard",
    "if",
    "implements",
    "import",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "let",
    "long",
    "namespace",
    "new",
    "nil",
    "null",
    "object",
    "override",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "string",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "throws",
    "true",
    "try",
    "using",
    "val",
    "var",
    "void",
    "when",
    "while",
];

impl Language {
    /// The language of the file at `path`, from its name; plain text when unknown.
    pub fn of(path: &Path) -> &'static Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let extension = path.extension().and_then(|extension| extension.to_str());
        LANGUAGES
            .iter()
            .find(|language| {
                language.files.contains(&name)
                    || extension.is_some_and(|extension| {
                        language
                            .extensions
                            .iter()
                            .any(|known| known.eq_ignore_ascii_case(extension))
                    })
            })
            .unwrap_or(&PLAIN)
    }

    pub fn plain() -> &'static Self {
        &PLAIN
    }

    /// The language called `name`, as the status bar shows it.
    pub fn named(name: &str) -> Option<&'static Self> {
        LANGUAGES.iter().find(|language| language.name == name)
    }

    /// The language a Markdown code fence names: by its name, or by the extension of its
    /// files.
    pub fn fenced(name: &str) -> &'static Self {
        let wanted = name.trim().to_lowercase();
        let wanted = match wanted.as_str() {
            "bash" | "shell" | "console" | "zsh" => "sh",
            "node" => "js",
            "c#" | "csharp" => "cs",
            "golang" => "go",
            "yml" => "yaml",
            other => other,
        };
        let named = |language: &&Language| language.name.eq_ignore_ascii_case(wanted);
        LANGUAGES
            .iter()
            .find(named)
            .unwrap_or_else(|| Self::of(Path::new(&format!("code.{wanted}"))))
    }

    /// How many languages are coloured.
    pub fn count() -> usize {
        LANGUAGES.len()
    }

    /// Whether `line`, a line of code up to its end, opens a block with a word: `if x
    /// then`, `def f`, `items.each do |item|`. What brackets and colons open is not told
    /// here.
    pub fn opens_block(&self, line: &str) -> bool {
        let line = line.trim();
        let last = line.rsplit(|ch: char| !ch.is_alphanumeric()).next();
        let last = last.unwrap_or("");
        let starts = |words: &[&str]| {
            let rest = words.iter().find_map(|word| line.strip_prefix(word));
            rest.is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '(']))
        };
        // A block closed on the line it opens is no block to be in.
        let closed = last == "end" || line.contains(" end ");
        match self.name {
            "Lua" => {
                let anonymous = line.ends_with(')') && line.contains("function(");
                let named = starts(&["function", "local function"]);
                !closed && (matches!(last, "then" | "do" | "else" | "repeat") || named || anonymous)
            }
            "Ruby" => {
                let with_arguments = line.ends_with('|') && line.contains(" do |");
                let worded = matches!(last, "do" | "then" | "else" | "begin" | "ensure" | "rescue");
                let opening = [
                    "def", "class", "module", "if", "unless", "while", "until", "case", "elsif",
                    "when",
                ];
                !closed && (worded || with_arguments || starts(&opening))
            }
            "Shell" => matches!(last, "then" | "do" | "else"),
            "Elixir" => !closed && (matches!(last, "do" | "else") || line.ends_with("->")),
            _ => false,
        }
    }

    /// The words that end a block, or start its next part: typed first on a line, they go
    /// back one level.
    pub fn block_closers(&self) -> &'static [&'static str] {
        match self.name {
            "Lua" => &["end", "else", "until"],
            "Ruby" => &["end", "else", "elsif", "when", "rescue", "ensure"],
            "Shell" => &["fi", "done", "else", "elif", "esac"],
            "Elixir" => &["end", "else"],
            _ => &[],
        }
    }

    /// Tags and attributes rather than statements: HTML, XML.
    pub fn is_markup(&self) -> bool {
        self.family == Family::Markup
    }

    /// Whether `text` begins with a marker that comments out the rest of its line.
    pub(crate) fn starts_line_comment(&self, text: &str) -> bool {
        self.line_comments
            .iter()
            .any(|prefix| text.starts_with(prefix))
    }

    /// The marker that comments out one line, when the language has one.
    pub fn line_comment(&self) -> Option<&'static str> {
        self.line_comments.first().map(|prefix| prefix.trim_end())
    }

    /// Whether unbalanced brackets are worth reporting in this language.
    pub fn checks_brackets(&self) -> bool {
        self.brackets
    }

    pub fn keywords(&self) -> &'static [&'static str] {
        self.keywords
    }

    fn is_keyword(&self, word: &str) -> bool {
        if self.keywords_ignore_case {
            self.keywords
                .iter()
                .any(|keyword| keyword.eq_ignore_ascii_case(word))
        } else {
            self.keywords.contains(&word)
        }
    }
}

/// Spans collected in order; neighbours of the same kind merge.
struct Spans(Vec<Span>);

impl Spans {
    /// Everything not covered yet, up to `end`, is `token`.
    fn push(&mut self, end: usize, token: Token) {
        let covered = self.0.last().map_or(0, |last| last.end);
        if end <= covered {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.token == token => last.end = end,
            _ => self.0.push(Span { end, token }),
        }
    }

    /// The bytes `start..end` are `token`; what lies before them is plain.
    fn mark(&mut self, start: usize, end: usize, token: Token) {
        self.push(start, Token::Plain);
        self.push(end, token);
    }
}

/// The colours written in a line of a style sheet or of a page: where each is, in chars,
/// and what it is, as red, green, blue and opacity. `#rgb`, `#rrggbb`, with or without
/// opacity, and `rgb(…)` / `rgba(…)` with numbers.
pub fn colours(line: &str) -> Vec<(std::ops::Range<usize>, [u8; 4])> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let rest: String = chars[at..].iter().take(40).collect();
        let named = at > 0 && (chars[at - 1].is_alphanumeric() || chars[at - 1] == '_');
        let read = match chars[at] {
            '#' if !named => hexadecimal(&rest),
            'r' if !named => in_numbers(&rest),
            _ => None,
        };
        match read {
            Some((length, colour)) => {
                found.push((at..at + length, colour));
                at += length;
            }
            None => at += 1,
        }
    }
    found
}

/// A colour in hexadecimal at the start of `text`, and the chars it takes.
fn hexadecimal(text: &str) -> Option<(usize, [u8; 4])> {
    let digits: Vec<u32> = text[1..].chars().map_while(|ch| ch.to_digit(16)).collect();
    let pair = |at: usize| (digits[at] * 16 + digits[at + 1]) as u8;
    let one = |at: usize| (digits[at] * 17) as u8;
    let colour = match digits.len() {
        3 => [one(0), one(1), one(2), 255],
        4 => [one(0), one(1), one(2), one(3)],
        6 => [pair(0), pair(2), pair(4), 255],
        8 => [pair(0), pair(2), pair(4), pair(6)],
        _ => return None,
    };
    Some((digits.len() + 1, colour))
}

/// A colour as `rgb(…)` or `rgba(…)` at the start of `text`, and the chars it takes.
fn in_numbers(text: &str) -> Option<(usize, [u8; 4])> {
    let inside = text.strip_prefix("rgba(").or(text.strip_prefix("rgb("))?;
    let (inside, _) = inside.split_once(')')?;
    let parts: Vec<&str> = inside
        .split([',', ' ', '/'])
        .filter(|part| !part.is_empty())
        .collect();
    let channel = |part: &str| {
        part.parse::<f32>()
            .ok()
            .map(|value| value.clamp(0.0, 255.0) as u8)
    };
    let opacity = match parts.get(3) {
        Some(part) => match part.strip_suffix('%') {
            Some(percent) => percent.parse::<f32>().ok()? / 100.0,
            None => part.parse::<f32>().ok()?,
        },
        None => 1.0,
    };
    let colour = [
        channel(parts.first()?)?,
        channel(parts.get(1)?)?,
        channel(parts.get(2)?)?,
        (opacity.clamp(0.0, 1.0) * 255.0) as u8,
    ];
    let length = text.chars().take_while(|ch| *ch != ')').count() + 1;
    (parts.len() == 3 || parts.len() == 4).then_some((length, colour))
}

/// What a line of code leaves open for the next one to go on with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Open {
    Nothing,
    /// A block comment.
    Comment,
    /// A Rust raw string, closed by a quote and this many hashes.
    Raw(usize),
    /// A string between three of this quote.
    Triple(u8),
    /// A string between two of this quote: one that runs over lines, or whose line end
    /// was escaped.
    Quoted(u8),
}

/// How the lexer of code stands at a line start.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Code {
    open: Open,
    /// The last byte of code seen: after an operator a `/` opens a regular expression,
    /// after a value it divides.
    last: u8,
}

impl Default for Code {
    fn default() -> Self {
        Self {
            open: Open::Nothing,
            last: b'(',
        }
    }
}

/// What a page holds in another language.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Inside {
    Script,
    Style,
}

impl Inside {
    fn language(self) -> Option<&'static Language> {
        Language::named(match self {
            Self::Script => "JavaScript",
            Self::Style => "CSS",
        })
    }

    /// What ends it, in lower case.
    fn closing(self) -> &'static str {
        match self {
            Self::Script => "</script",
            Self::Style => "</style",
        }
    }
}

/// How the lexer of a page stands at a line start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Page {
    #[default]
    Text,
    Comment,
    /// In a tag: whether its name is read, the quote of the value being read, and what
    /// the tag opens once closed.
    Tag {
        named: bool,
        quote: Option<u8>,
        opens: Option<Inside>,
    },
    /// In a script or a style, read as code.
    Inside(Inside),
}

/// The language a Markdown fence names: the same when it is the same one of the table.
#[derive(Clone, Copy, Debug)]
struct Fence(&'static Language);

impl PartialEq for Fence {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

/// How the lexer stands at a line start: all that the lines before leave to the ones
/// after. Each family reads its own part; a fence and a page hand the rest to the
/// language they hold.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct State {
    /// The Markdown fence being read.
    fence: Option<Fence>,
    page: Page,
    code: Code,
}

/// Colour `text`: spans in order, covering it entirely.
pub fn highlight(text: &str, language: &Language) -> Vec<Span> {
    let mut spans = Spans(Vec::new());
    let (mut start, mut state) = (0, State::default());
    while start < text.len() {
        let end = line_end(text, start);
        state = lex_line(text, start, end, language, state, &mut spans);
        start = end;
    }
    spans.push(text.len(), Token::Plain);
    spans.0
}

/// What is known of a text once coloured, kept to colour it again after a change without
/// reading it all.
#[derive(Clone, Debug, PartialEq)]
pub struct Coloured {
    spans: Vec<Span>,
    /// Where each line starts, and how the lexer stood there.
    lines: Vec<(usize, State)>,
    /// The bytes read to colour the text last.
    read: usize,
}

impl Coloured {
    /// Colour a whole text.
    pub fn new(text: &str, language: &Language) -> Coloured {
        let mut coloured = Coloured {
            spans: Vec::new(),
            lines: Vec::new(),
            read: 0,
        };
        // Nothing, into which the whole text was written.
        coloured.edit(text, language, 0..0, 0..text.len());
        coloured
    }

    /// The spans, as `highlight` gives them.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Colour again after a change: the bytes `old` of the text this was made for were
    /// replaced, and now stand as the bytes `new` of `text` (the text after the change).
    /// Only the lines from the change down to where the colouring falls back in step with
    /// what was known are read. Ranges that do not tell such a change have the whole text
    /// coloured again.
    pub fn edit(
        &mut self,
        text: &str,
        language: &Language,
        old: std::ops::Range<usize>,
        new: std::ops::Range<usize>,
    ) {
        let before = self.spans.last().map_or(0, |span| span.end);
        // The line of the byte before the change: the line changed, or the one above when
        // the change starts a line, as what ends a line can be told by what follows it.
        let above = self.lines.partition_point(|line| line.0 < new.start);
        let first = above.saturating_sub(1);
        let (restart, mut state) = self.lines.get(first).copied().unwrap_or_default();
        let fits = old.start == new.start
            && old.start <= old.end
            && old.end <= before
            && new.start <= new.end
            && new.end <= text.len()
            && before - old.end == text.len() - new.end
            && (restart == 0 || text.as_bytes().get(restart - 1) == Some(&b'\n'));
        if !fits {
            *self = Coloured::new(text, language);
            return;
        }
        // The span that holds the byte before `restart` is the first one made again.
        let from = self.spans.partition_point(|span| span.end < restart);
        let mut spans = Spans(Vec::new());
        if let Some(span) = self.spans.get(from).filter(|_| restart > 0) {
            spans.push(restart, span.token);
        }
        let mut lines = Vec::new();
        // `known` is the line known before the change that the one at `start` is now.
        let (mut start, mut known, mut met) = (restart, first, false);
        while !met && start < text.len() {
            lines.push((start, state));
            let end = line_end(text, start);
            state = lex_line(text, start, end, language, state, &mut spans);
            start = end;
            if start >= new.end {
                // Past the change, a line at whose start the lexer stands as it stood:
                // what follows keeps its colours.
                let was = start - new.end + old.end;
                while self.lines.get(known).is_some_and(|line| line.0 < was) {
                    known += 1;
                }
                met = self.lines.get(known) == Some(&(was, state));
            }
        }
        spans.push(start, Token::Plain);
        let was = if met {
            start - new.end + old.end
        } else {
            known = self.lines.len();
            before
        };
        // The span cut at the line met keeps its end.
        let mut to = self.spans.partition_point(|span| span.end <= was);
        if let Some(span) = self.spans.get(to) {
            spans.push(span.end - old.end + new.end, span.token);
            to += 1;
        }
        let (spanned, lined) = (spans.0.len(), lines.len());
        self.spans.splice(from..to, spans.0);
        self.lines.splice(first..known, lines);
        // What follows moves along.
        for span in &mut self.spans[from + spanned..] {
            span.end = span.end - old.end + new.end;
        }
        for line in &mut self.lines[first + lined..] {
            line.0 = line.0 - old.end + new.end;
        }
        self.read = start - restart;
    }

    /// How many bytes of the text were read to colour it last: all of them by `new`, the
    /// lines gone through by `edit`.
    pub fn last_read(&self) -> usize {
        self.read
    }
}

fn is_word_byte(byte: u8) -> bool {
    // Bytes of non-ASCII letters count as word bytes, which also keeps offsets on char bounds.
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn find_from(text: &str, from: usize, needle: &str) -> Option<usize> {
    text.get(from..)?.find(needle).map(|index| from + index)
}

/// Where the line that starts at byte `start` ends: past its line end, or with the text.
fn line_end(text: &str, start: usize) -> usize {
    let rest = text.get(start..).unwrap_or("");
    rest.find('\n').map_or(text.len(), |at| start + at + 1)
}

/// Colour the line `from..to` of `text`, its line end included, the lexer standing as
/// `state` at its start: how it stands at its end.
fn lex_line(
    text: &str,
    from: usize,
    to: usize,
    language: &Language,
    state: State,
    spans: &mut Spans,
) -> State {
    match language.family {
        Family::Text => state,
        Family::Code => State {
            code: code(text, from, to, language, state.code, spans),
            ..state
        },
        Family::Markup => markup(text, from, to, state, spans),
        Family::Markdown => markdown(text, from, to, state, spans),
    }
}

/// Where the string opened by `quote` closes in `line`, read from byte `from` inside it;
/// `None` when it runs past the line.
fn quoted_end(line: &str, from: usize, quote: u8, multiline: bool) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            byte if byte == quote => return Some(index + 1),
            b'\n' if !multiline => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

/// Where what is `open` closes in `line`, looked for from byte `from` inside it; `None`
/// when it runs past the line.
fn closing(line: &str, from: usize, open: Open, language: &Language) -> Option<usize> {
    let past = |close: &str| find_from(line, from, close).map(|found| found + close.len());
    match open {
        Open::Nothing => Some(from),
        Open::Comment => past(language.block_comment?.1),
        Open::Raw(hashes) => past(&format!("\"{}", "#".repeat(hashes))),
        Open::Triple(quote) => {
            let triple = [quote; 3];
            past(std::str::from_utf8(&triple).unwrap_or("\"\"\""))
        }
        Open::Quoted(quote) => quoted_end(line, from, quote, language.multiline.contains(&quote)),
    }
}

/// In Rust `'a'` and `'\n'` are chars, `'a` is a lifetime.
fn is_char_literal(rest: &str) -> bool {
    let mut chars = rest.chars().skip(1);
    match chars.next() {
        Some('\\') => true,
        Some(_) => chars.next() == Some('\''),
        None => false,
    }
}

/// The Rust raw string (`r"…"`, `r#"…"#`, `br"…"`) that `rest` starts with: its hashes,
/// and the bytes its opening takes.
fn raw_string(rest: &str) -> Option<(usize, usize)> {
    let marked = rest.strip_prefix('b').unwrap_or(rest).strip_prefix('r')?;
    let hashes = marked.len() - marked.trim_start_matches('#').len();
    let body = marked[hashes..].strip_prefix('"')?;
    Some((hashes, rest.len() - body.len()))
}

/// End of the regular expression literal opened by the `/` at `start`, when it closes on
/// its line.
fn regex_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut in_class = false;
    while let Some(byte) = bytes.get(index) {
        match byte {
            b'\n' => return None,
            b'\\' => index += 1,
            b'[' => in_class = true,
            b']' => in_class = false,
            b'/' if !in_class => {
                let mut end = index + 1;
                while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                    end += 1;
                }
                return Some(end);
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Colour the bytes `from..to` of `text` as code, `to` ending a line or the text, the
/// lexer standing as `state` at `from`: how it stands at `to`.
fn code(
    text: &str,
    from: usize,
    to: usize,
    language: &Language,
    state: Code,
    spans: &mut Spans,
) -> Code {
    let bytes = text.as_bytes();
    // No token is looked for past the line; only a `'` looks one char into the next.
    let line = &text[..to];
    let mut index = from;
    let mut last = state.last;
    let mut left = state.open;
    // Only where a `/` can open a regular expression does the last byte matter.
    let stands = |open: Open, last: u8| Code {
        open,
        last: if language.regex_literals { last } else { b'(' },
    };
    while index < to {
        let byte = bytes[index];
        let rest = &text[index..];
        let lifetime = language.rust && byte == b'\'' && !is_char_literal(rest);
        // What starts here: its kind, and either its end, or what it opens and where the
        // search for its close starts.
        let (token, open, at) = if left != Open::Nothing {
            // What the line before left open goes on.
            let token = if left == Open::Comment {
                Token::Comment
            } else {
                Token::String
            };
            (token, std::mem::replace(&mut left, Open::Nothing), index)
        } else if language
            .line_comments
            .iter()
            .any(|prefix| rest.starts_with(prefix))
        {
            let end = find_from(line, index, "\n").unwrap_or(to);
            (Token::Comment, Open::Nothing, end)
        } else if let Some((opening, _)) = language.block_comment
            && rest.starts_with(opening)
        {
            (Token::Comment, Open::Comment, index + opening.len())
        } else if language.rust
            && let Some((hashes, length)) = raw_string(rest)
        {
            (Token::String, Open::Raw(hashes), index + length)
        } else if language.regex_literals
            && byte == b'/'
            && b"(,=:[!&|?{};".contains(&last)
            && let Some(end) = regex_end(line.as_bytes(), index)
        {
            (Token::String, Open::Nothing, end)
        } else if language.quotes.contains(&byte) && !lifetime {
            if language.triple_quotes && bytes[index..].starts_with(&[byte; 3]) {
                (Token::String, Open::Triple(byte), index + 3)
            } else {
                (Token::String, Open::Quoted(byte), index + 1)
            }
        } else if byte.is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || bytes[end] == b'_'
                    || (bytes[end] == b'.' && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)))
            {
                end += 1;
            }
            (Token::Number, Open::Nothing, end)
        } else if is_word_byte(byte) {
            let mut end = index + 1;
            while end < bytes.len() && is_word_byte(bytes[end]) {
                end += 1;
            }
            let word = &text[index..end];
            let after = text[end..].trim_start_matches(' ').as_bytes().first();
            let token = if language.is_keyword(word) {
                Token::Keyword
            } else if after == Some(&b'(') || (language.rust && bytes.get(end) == Some(&b'!')) {
                Token::Function
            } else if byte.is_ascii_uppercase() {
                Token::Type
            } else {
                Token::Plain
            };
            (token, Open::Nothing, end)
        } else {
            if !byte.is_ascii_whitespace() {
                last = byte;
            }
            index += 1;
            continue;
        };
        let Some(end) = closing(line, at, open, language) else {
            spans.mark(index, to, token);
            return stands(open, last);
        };
        if token != Token::Comment {
            // A keyword (`return`) is followed by a value, as an operator is.
            last = if token == Token::Keyword {
                b'('
            } else {
                bytes[end.max(1) - 1]
            };
        }
        spans.mark(index, end, token);
        index = end;
    }
    stands(Open::Nothing, last)
}

/// HTML / XML: comments, tag names, attribute names and values; scripts and styles in
/// their own language. Colour the line `from..to` of `text`, the lexer standing as
/// `state` at its start: how it stands at its end.
fn markup(text: &str, from: usize, to: usize, mut state: State, spans: &mut Spans) -> State {
    let line = &text[..to];
    let bytes = line.as_bytes();
    let mut at = from;
    while at < to {
        match state.page {
            Page::Text => {
                let Some(open) = find_from(line, at, "<") else {
                    break;
                };
                spans.push(open, Token::Plain);
                if line[open..].starts_with("<!--") {
                    state.page = Page::Comment;
                    at = open + 4;
                } else {
                    let name = &bytes[open + 1..];
                    let length = name.iter().take_while(|byte| byte.is_ascii_alphanumeric());
                    let name = &name[..length.count()];
                    let opens = if name.eq_ignore_ascii_case(b"script") {
                        Some(Inside::Script)
                    } else if name.eq_ignore_ascii_case(b"style") {
                        Some(Inside::Style)
                    } else {
                        None
                    };
                    state.page = Page::Tag {
                        named: false,
                        quote: None,
                        opens,
                    };
                    at = open + 1;
                }
            }
            Page::Comment => match find_from(line, at, "-->") {
                Some(found) => {
                    spans.push(found + 3, Token::Comment);
                    state.page = Page::Text;
                    at = found + 3;
                }
                None => at = to,
            },
            Page::Tag {
                named,
                quote: Some(quote),
                opens,
            } => {
                // A value ends with its quote, or with the tag.
                let ends = |byte: &u8| *byte == quote || *byte == b'>';
                let Some(found) = bytes[at..].iter().position(ends) else {
                    break;
                };
                at += found + usize::from(bytes[at + found] == quote);
                spans.push(at, Token::String);
                state.page = Page::Tag {
                    named,
                    quote: None,
                    opens,
                };
            }
            Page::Tag {
                named,
                quote: None,
                opens,
            } => {
                let byte = bytes[at];
                if byte == b'>' {
                    // `<script/>` holds nothing.
                    let whole = at > 0 && bytes[at - 1] == b'/';
                    let opens = opens.filter(|inside| !whole && inside.language().is_some());
                    state.page = opens.map_or(Page::Text, Page::Inside);
                    state.code = Code::default();
                    at += 1;
                } else if byte == b'"' || byte == b'\'' {
                    spans.push(at, Token::Plain);
                    state.page = Page::Tag {
                        named,
                        quote: Some(byte),
                        opens,
                    };
                    at += 1;
                } else if is_word_byte(byte) {
                    let mut end = at + 1;
                    while end < to && (is_word_byte(bytes[end]) || b"-:.".contains(&bytes[end])) {
                        end += 1;
                    }
                    let token = if named { Token::Type } else { Token::Keyword };
                    spans.mark(at, end, token);
                    state.page = Page::Tag {
                        named: true,
                        quote: None,
                        opens,
                    };
                    at = end;
                } else {
                    at += 1;
                }
            }
            Page::Inside(inside) => {
                let Some(language) = inside.language() else {
                    state.page = Page::Text;
                    continue;
                };
                let closing = inside.closing().as_bytes();
                let closes = |window: &[u8]| window.eq_ignore_ascii_case(closing);
                match bytes[at..].windows(closing.len()).position(closes) {
                    // The code ends where its closing tag starts, whatever it left open.
                    Some(found) => {
                        let end = at + found;
                        code(&text[..end], at, end, language, state.code, spans);
                        state.page = Page::Text;
                        at = end;
                    }
                    None => {
                        state.code = code(text, at, to, language, state.code, spans);
                        at = to;
                    }
                }
            }
        }
    }
    // What is left open runs to the end of the line.
    match state.page {
        Page::Comment => spans.push(to, Token::Comment),
        Page::Tag { quote: Some(_), .. } => spans.push(to, Token::String),
        _ => {}
    }
    state
}

/// Markdown: headings, quotes, fenced and inline code. Colour the line `from..to` of
/// `text`, the lexer standing as `state` at its start: how it stands at its end.
fn markdown(text: &str, from: usize, to: usize, state: State, spans: &mut Spans) -> State {
    let line = &text[from..to];
    let line = line.strip_suffix('\n').unwrap_or(line);
    let end = from + line.len();
    let trimmed = line.trim_start();
    if let Some(named) = trimmed.strip_prefix("```") {
        spans.mark(from, end, Token::String);
        // A fence closes the one being read, or opens one in the language it names.
        return State {
            fence: match state.fence {
                Some(_) => None,
                None => Some(Fence(Language::fenced(named))),
            },
            ..State::default()
        };
    }
    if let Some(Fence(language)) = state.fence {
        if language.family == Family::Text {
            spans.mark(from, to, Token::String);
            return state;
        }
        // Its language reads the line, knowing of no fence.
        let within = State {
            fence: None,
            ..state
        };
        return State {
            fence: state.fence,
            ..lex_line(text, from, to, language, within, spans)
        };
    }
    if trimmed.starts_with('#') {
        spans.mark(from, end, Token::Keyword);
    } else if trimmed.starts_with('>') {
        spans.mark(from, end, Token::Comment);
    } else {
        let line = &text[..end];
        let mut at = from;
        while let Some(open) = find_from(line, at, "`") {
            let Some(close) = find_from(line, open + 1, "`") else {
                break;
            };
            spans.mark(open, close + 1, Token::String);
            at = close + 1;
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_colours_of_a_line_are_found() {
        let line = "a { color: #fff; background: #3498DB80; border: 1px solid rgb(10, 20, 300) }";
        let found = colours(line);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0], (11..15, [255, 255, 255, 255]));
        assert_eq!(found[1], (29..38, [0x34, 0x98, 0xDB, 0x80]));
        assert_eq!(found[2].1, [10, 20, 255, 255]);
        assert_eq!(&line[found[2].0.clone()], "rgb(10, 20, 300)");
        assert_eq!(colours("rgba(0 0 0 / 50%)")[0].1, [0, 0, 0, 127]);
        // An id, a word, a number of digits that makes no colour: none.
        assert!(colours("#header { margin: 0 } grgb(1,2,3) #12 #12345 x#fff").is_empty());
    }

    /// The text of every non-plain span, with its kind.
    fn tokens<'a>(text: &'a str, file: &str) -> Vec<(&'a str, Token)> {
        let spans = highlight(text, Language::of(Path::new(file)));
        assert_eq!(spans.last().map(|span| span.end), Some(text.len()));
        let mut start = 0;
        let mut found = Vec::new();
        for span in spans {
            assert!(span.end > start, "spans grow");
            if span.token != Token::Plain {
                found.push((&text[start..span.end], span.token));
            }
            start = span.end;
        }
        found
    }

    #[test]
    fn rust_is_coloured() {
        let text = "fn main<'a>() { let x: Vec<u8> = vec![b'\\n', 42]; // done\n}";
        assert_eq!(
            tokens(text, "main.rs"),
            [
                ("fn", Token::Keyword),
                ("let", Token::Keyword),
                ("Vec", Token::Type),
                ("vec", Token::Function),
                ("'\\n'", Token::String),
                ("42", Token::Number),
                ("// done", Token::Comment),
            ]
        );
    }

    #[test]
    fn strings_stop_at_the_line_end_unless_multiline() {
        assert_eq!(
            tokens("a = 'open\nb = 1", "x.py"),
            [("'open", Token::String), ("1", Token::Number)]
        );
        assert_eq!(
            tokens("\"\"\"doc\nstring\"\"\" pass", "x.py"),
            [
                ("\"\"\"doc\nstring\"\"\"", Token::String),
                ("pass", Token::Keyword)
            ]
        );
        assert_eq!(
            tokens("`a\nb` /* c\nd */ é(1..2)", "x.ts"),
            [
                ("`a\nb`", Token::String),
                ("/* c\nd */", Token::Comment),
                ("é", Token::Function),
                ("1", Token::Number),
                ("2", Token::Number),
            ]
        );
    }

    #[test]
    fn markup_and_markdown_are_coloured() {
        assert_eq!(
            tokens("<!-- x --><a href=\"u\">t</a>", "index.html"),
            [
                ("<!-- x -->", Token::Comment),
                ("a", Token::Keyword),
                ("href", Token::Type),
                ("\"u\"", Token::String),
                ("a", Token::Keyword),
            ]
        );
        assert_eq!(
            tokens("# Title\nuse `code` here\n> quote", "README.md"),
            [
                ("# Title", Token::Keyword),
                ("`code`", Token::String),
                ("> quote", Token::Comment),
            ]
        );
    }

    #[test]
    fn code_inside_pages_and_fences_takes_its_own_colours() {
        assert_eq!(
            tokens(
                "<style>a{}/* c */</style><script>let x = 'y';</script><b>let</b>",
                "a.html"
            ),
            [
                ("style", Token::Keyword),
                ("/* c */", Token::Comment),
                ("style", Token::Keyword),
                ("script", Token::Keyword),
                ("let", Token::Keyword),
                ("'y'", Token::String),
                ("script", Token::Keyword),
                ("b", Token::Keyword),
                ("b", Token::Keyword),
            ]
        );
        assert_eq!(
            tokens("```rust\nfn a() {}\n```\ntext\n```\nraw", "a.md"),
            [
                ("```rust", Token::String),
                ("fn", Token::Keyword),
                ("a", Token::Function),
                ("```", Token::String),
                ("```", Token::String),
                ("raw", Token::String),
            ]
        );
    }

    #[test]
    fn many_languages_are_known() {
        assert!(Language::count() >= 70, "{} languages", Language::count());
        for (file, name) in [
            ("a.zig", "Zig"),
            ("Dockerfile", "Dockerfile"),
            ("main.tf", "HCL"),
            ("a.sol", "Solidity"),
            ("CMakeLists.txt", "CMake"),
            ("a.sass", "CSS"),
        ] {
            assert_eq!(Language::of(Path::new(file)).name, name);
        }
        assert_eq!(Language::fenced("C++").name, "C++");
        assert_eq!(Language::fenced("bash").name, "Shell");
        assert_eq!(Language::fenced("py").name, "Python");
        assert_eq!(Language::fenced("").name, "Plain Text");
        assert_eq!(
            tokens("x = 1 ! note", "a.f90"),
            [("1", Token::Number), ("! note", Token::Comment)]
        );
    }

    #[test]
    fn languages_are_found_by_name() {
        assert_eq!(Language::of(Path::new("a/b/Main.RS")).name, "Rust");
        assert_eq!(Language::of(Path::new("Makefile")).name, "Shell");
        assert_eq!(Language::of(Path::new("notes")).name, "Plain Text");
        assert_eq!(Language::of(Path::new("x.sql")).line_comment(), Some("--"));
        assert_eq!(Language::of(Path::new("x.css")).line_comment(), None);
        assert!(tokens("SELECT 1", "x.sql").contains(&("SELECT", Token::Keyword)));
    }

    /// Every language of the table, and plain text.
    fn languages() -> impl Iterator<Item = &'static Language> {
        LANGUAGES.iter().chain(std::iter::once(&PLAIN))
    }

    /// Replace the bytes `at` of `text` by `with`, have `coloured` follow, and check that
    /// it knows what colouring the whole text anew gives.
    fn change(
        coloured: &mut Coloured,
        text: &mut String,
        language: &Language,
        at: std::ops::Range<usize>,
        with: &str,
    ) {
        let before = text.clone();
        text.replace_range(at.clone(), with);
        coloured.edit(text, language, at.clone(), at.start..at.start + with.len());
        let whole = Coloured::new(text, language);
        assert!(
            coloured.spans() == highlight(text, language) && coloured.lines == whole.lines,
            "{}: {at:?} replaced by {with:?}\nbefore: {before:?}\nafter: {text:?}\nkept: {:?}\nwhole: {:?}",
            language.name,
            coloured.spans(),
            whole.spans(),
        );
    }

    /// `before`, as the file `file`, once its bytes `at` are replaced by `with`: the text,
    /// and its colours, which followed the change.
    fn changed(
        file: &str,
        before: &str,
        at: std::ops::Range<usize>,
        with: &str,
    ) -> (String, Coloured) {
        let language = Language::of(Path::new(file));
        let mut text = before.to_string();
        let mut coloured = Coloured::new(&text, language);
        assert_eq!(coloured.spans(), highlight(&text, language));
        change(&mut coloured, &mut text, language, at, with);
        (text, coloured)
    }

    /// The token of the byte `at`.
    fn token_at(coloured: &Coloured, at: usize) -> Token {
        let spans = coloured.spans();
        spans[spans.partition_point(|span| span.end <= at)].token
    }

    #[test]
    fn typing_inside_a_line_reads_that_line_alone() {
        let text = "fn a() {}\nlet x = 1;\nlet y = 2;\n";
        let at = text.find('x').unwrap();
        let (_, coloured) = changed("a.rs", text, at..at, "z");
        assert_eq!(coloured.last_read(), "let zx = 1;\n".len());
        // Typed first on a line, it has the line above read too: a `'` ending it is told
        // by what follows.
        let at = text.find("let y").unwrap();
        let (_, coloured) = changed("a.rs", text, at..at, "z");
        assert_eq!(coloured.last_read(), "let x = 1;\nzlet y = 2;\n".len());
    }

    #[test]
    fn a_block_comment_opened_takes_what_follows_and_closed_gives_it_back() {
        let language = Language::of(Path::new("a.c"));
        let mut text = "int a;\nint b;\nint c; /* old */\nint d;\nint e;\n".to_string();
        let mut coloured = Coloured::new(&text, language);
        let b = text.find("int b").unwrap();
        // Opened above a comment, it ends where that one ended: the lines after are not read.
        change(&mut coloured, &mut text, language, b..b, "/*");
        assert_eq!(
            token_at(&coloured, text.find("int c").unwrap()),
            Token::Comment
        );
        assert_eq!(
            token_at(&coloured, text.find("int d").unwrap()),
            Token::Keyword
        );
        let unread = "int d;\nint e;\n".len();
        assert_eq!(coloured.last_read(), text.len() - unread);
        // Closed at once, it gives back the lines it took, down to the old comment.
        change(&mut coloured, &mut text, language, b + 2..b + 2, "*/");
        assert_eq!(
            token_at(&coloured, text.find("int b").unwrap()),
            Token::Keyword
        );
        assert_eq!(
            token_at(&coloured, text.find("old").unwrap()),
            Token::Comment
        );
        assert_eq!(coloured.last_read(), "/**/int b;\nint c; /* old */\n".len());
        // With nothing to close it below, it takes all the rest: read from the line above
        // the one it starts, down to the end.
        let e = text.find("int e").unwrap();
        let d = text.find("int d").unwrap();
        change(&mut coloured, &mut text, language, d..d, "/*");
        assert_eq!(token_at(&coloured, e + 2), Token::Comment);
        assert_eq!(
            coloured.last_read(),
            text.len() - text.find("int c").unwrap()
        );
        // Deleted, it gives it all back.
        change(&mut coloured, &mut text, language, d..d + 2, "");
        assert_eq!(token_at(&coloured, e), Token::Keyword);
    }

    #[test]
    fn a_string_left_open_swallows_the_lines_that_follow() {
        // A template literal runs to the next backtick, which then opens another.
        let text = "let a = 1;\nlet b = 2;\nlet c = `t`;\nlet d = 4;\n";
        let (after, coloured) = changed("a.js", text, 8..8, "`");
        assert_eq!(
            token_at(&coloured, after.find("let b").unwrap()),
            Token::String
        );
        assert_eq!(token_at(&coloured, after.find("t`").unwrap()), Token::Plain);
        assert_eq!(
            token_at(&coloured, after.find("let d").unwrap()),
            Token::String
        );
        // Three quotes take the lines down to the next three.
        let text = "a = 1\nb = 2\nc = \"\"\"doc\"\"\"\nd = 4\n";
        let (after, coloured) = changed("a.py", text, 0..0, "\"\"\"");
        assert_eq!(
            token_at(&coloured, after.find("b = 2").unwrap()),
            Token::String
        );
        assert_eq!(
            token_at(&coloured, after.find("doc").unwrap()),
            Token::Plain
        );
        // A raw string of Rust ends with as many hashes as opened it.
        let text = "let a = \"x\";\nlet b = 2; // \"# y\nlet c = 3;\n";
        let (after, coloured) = changed("a.rs", text, 8..8, "r#");
        assert_eq!(
            token_at(&coloured, after.find("let b").unwrap()),
            Token::String
        );
        assert_eq!(
            token_at(&coloured, after.find("let c").unwrap()),
            Token::Keyword
        );
        // A line end escaped in a string has it go on with the next line.
        let text = "s = \"one\nint two;\"\nint three;\n";
        let (after, coloured) = changed("a.c", text, 8..8, "\\");
        assert_eq!(
            token_at(&coloured, after.find("int two").unwrap()),
            Token::String
        );
        assert_eq!(
            token_at(&coloured, after.find("int three").unwrap()),
            Token::Keyword
        );
        let (after, coloured) = changed("a.c", &after, 8..9, "");
        assert_eq!(
            token_at(&coloured, after.find("int two").unwrap()),
            Token::Keyword
        );
    }

    #[test]
    fn deleting_a_line_end_joins_two_lines() {
        let text = "a = 1 // note\nb = 2\nc = 3\n";
        let end = text.find('\n').unwrap();
        let (after, coloured) = changed("a.c", text, end..end + 1, "");
        assert_eq!(after, "a = 1 // noteb = 2\nc = 3\n");
        assert_eq!(
            token_at(&coloured, after.find('2').unwrap()),
            Token::Comment
        );
        assert_eq!(token_at(&coloured, after.find('3').unwrap()), Token::Number);
        assert_eq!(coloured.last_read(), "a = 1 // noteb = 2\n".len());
        // And the last one, which leaves a text without a line end.
        let (after, _) = changed("a.c", text, text.len() - 1..text.len(), "");
        assert!(after.ends_with('3'));
    }

    #[test]
    fn a_text_is_changed_at_its_ends_and_as_a_whole() {
        // Written into nothing, and emptied.
        let (after, coloured) = changed("a.rs", "", 0..0, "fn a() {}");
        assert_eq!(coloured.last_read(), after.len());
        let (_, coloured) = changed("a.rs", "fn a() {}\n// b\n", 0..15, "");
        assert_eq!((coloured.spans(), coloured.last_read()), (&[][..], 0));
        let text = "let a = 1;\nlet b = 2;\n";
        // At its start.
        changed("a.rs", text, 0..0, "/* c */ ");
        changed("a.rs", text, 0..3, "");
        changed("a.rs", text, 0..0, "\n");
        // At its end, after a line end and after none.
        let (_, coloured) = changed("a.rs", text, text.len()..text.len(), "let c = \"");
        assert_eq!(coloured.last_read(), "let b = 2;\nlet c = \"".len());
        let (_, coloured) = changed("a.rs", "let a = 1;\nlet b", 16..16, " = 2;");
        assert_eq!(coloured.last_read(), "let b = 2;".len());
        changed("a.rs", text, text.len()..text.len(), "\n\n");
        // All of it replaced by another text, and by itself.
        let (_, coloured) = changed("a.rs", text, 0..text.len(), "/* all\nnew */\nfn b() {}");
        assert_eq!(coloured.last_read(), "/* all\nnew */\nfn b() {}".len());
        changed("a.rs", text, 0..text.len(), text);
    }

    #[test]
    fn chars_of_several_bytes_stand_around_a_change() {
        let text = "let é = \"日本\"; // ünï\nlet b = 'é'; /* 語\n語 */ let ñ = 2;\n";
        for (at, _) in text.char_indices() {
            let next = at + text[at..].chars().next().map_or(0, char::len_utf8);
            changed("a.rs", text, at..at, "语");
            changed("a.rs", text, at..next, "");
            changed("a.rs", text, at..next, "\"é");
            changed("a.md", text, at..next, "`ü");
            changed("a.html", text, at..next, "<é a='日");
        }
    }

    #[test]
    fn lines_ending_in_crlf_follow_a_change() {
        let text = "let a = 'x\\\r\ny';\r\n/* c\r\nd */ let b = `e\r\nf`;\r\n// g\r\nh();\r\n";
        for (at, _) in text.char_indices() {
            changed("a.js", text, at..at, "\r\n");
            changed("a.js", text, at..at + 1, "");
            changed("a.js", text, at..at, "`");
            changed("a.rs", text, at..at, "'");
        }
        let page = "<a\r\n href=\"u\r\nv\">\r\n<script>\r\nlet x = 1;\r\n</script>\r\n```rust\r\nfn a() {}\r\n```\r\n";
        for (at, _) in page.char_indices() {
            changed("a.html", page, at..at + 1, "");
            changed("a.md", page, at..at + 1, "");
            changed("a.md", page, at..at, "\r\n```\r\n");
        }
    }

    #[test]
    fn a_change_of_nothing_changes_nothing() {
        let text = "fn a() {}\n/* b\nc */ let d = 1;\n";
        for at in 0..=text.len() {
            let language = Language::of(Path::new("a.rs"));
            let whole = Coloured::new(text, language);
            let mut coloured = whole.clone();
            coloured.edit(text, language, at..at, at..at);
            assert_eq!(
                (coloured.spans(), &coloured.lines),
                (whole.spans(), &whole.lines)
            );
            assert!(coloured.last_read() <= "/* b\nc */ let d = 1;\n".len());
        }
    }

    #[test]
    fn a_quote_that_ends_a_line_is_told_by_the_line_below() {
        // `'` then a line end then `'` is a char in Rust; without the last, a lifetime.
        let text = "a '\nb\n";
        let (after, coloured) = changed("a.rs", text, 4..4, "'");
        assert_eq!(after, "a '\n'b\n");
        assert_eq!(
            token_at(&Coloured::new(text, Language::of(Path::new("a.rs"))), 2),
            Token::Plain
        );
        assert_eq!(token_at(&coloured, 2), Token::String);
        changed("a.rs", &after, 4..5, "");
        // In a fence too.
        let fenced = "```rust\na '\nb\n```\n";
        changed("a.md", fenced, 12..12, "'");
        changed("a.md", "```rust\na '\n'b\n```\n", 12..13, "");
    }

    #[test]
    fn ranges_that_tell_no_change_have_everything_coloured_again() {
        let language = Language::of(Path::new("a.rs"));
        let text = "let a = 1;\n/* b */\n";
        let whole = Coloured::new(text, language);
        assert_eq!(whole.last_read(), text.len());
        // Not the same start, a length that does not add up, ranges past the texts or
        // turned around.
        #[allow(clippy::reversed_empty_ranges)]
        let wrong = [
            (0..1, 2..3),
            (0..0, 0..1),
            (4..9, 4..4),
            (3..90, 3..90),
            (9..2, 9..2),
        ];
        for (old, new) in wrong {
            let mut coloured = Coloured::new("fn other() {}\nlet z = \"", language);
            coloured.edit(text, language, old, new);
            assert_eq!(coloured, whole);
        }
    }

    #[test]
    fn a_page_follows_a_change_in_its_tags_and_in_what_they_hold() {
        let page =
            "<p class=\"a\nb\">t</p>\n<script>\nlet x = `a\n`; /* c */\n</script>\n<b>let</b>\n";
        // Typed in the script: only its line is read.
        let x = page.find('x').unwrap();
        let (_, coloured) = changed("a.html", page, x..x, "y");
        assert_eq!(coloured.last_read(), "let yx = `a\n".len());
        // The script closed early: what it held is a page again.
        let (after, coloured) = changed("a.html", page, x..x, "</script>");
        assert_eq!(
            token_at(&coloured, after.find("let").unwrap()),
            Token::Keyword
        );
        assert_eq!(
            token_at(&coloured, after.find("/* c").unwrap()),
            Token::Plain
        );
        // Its closing tag gone: the rest of the page is script.
        let close = page.find("</script>").unwrap();
        let (after, coloured) = changed("a.html", page, close..close + 9, "");
        assert_eq!(
            token_at(&coloured, after.rfind("let").unwrap()),
            Token::Keyword
        );
        // A tag that holds nothing, a style, a comment, a value left open.
        let open = page.find("<script>").unwrap();
        changed("a.html", page, open + 7..open + 7, "/");
        changed("a.html", page, open + 1..open + 7, "STYLE");
        changed("a.html", page, 0..0, "<!--");
        let (after, coloured) = changed("a.html", page, 11..12, "");
        assert_eq!(
            token_at(&coloured, after.find("b\">t").unwrap()),
            Token::String
        );
        let quote = page.find("b\"").unwrap() + 1;
        let (after, coloured) = changed("a.html", page, quote..quote + 1, "");
        assert_eq!(
            token_at(&coloured, after.find("t</p").unwrap()),
            Token::Plain
        );
    }

    #[test]
    fn fences_follow_a_change() {
        let notes = "# T\n```rust\nlet s = \"a\nb\"; // c\n```\ntext `code`\n```\nraw\n";
        // Typed in a fence: only its line is read.
        let s = notes.find("s =").unwrap();
        let (_, coloured) = changed("a.md", notes, s..s, "x");
        assert_eq!(coloured.last_read(), "let xs = \"a\n".len());
        // Another language named: the fence is read again, and no more.
        let (after, coloured) = changed("a.md", notes, 7..11, "py");
        assert_eq!(
            token_at(&coloured, after.find("let").unwrap()),
            Token::Plain
        );
        assert_eq!(
            coloured.last_read(),
            "```py\nlet s = \"a\nb\"; // c\n```\n".len()
        );
        // A fence opened above: every fence below turns inside out.
        let (after, coloured) = changed("a.md", notes, 4..4, "```\n");
        assert_eq!(
            token_at(&coloured, after.find("let").unwrap()),
            Token::Plain
        );
        assert_eq!(
            token_at(&coloured, after.find("text").unwrap()),
            Token::String
        );
        assert_eq!(
            token_at(&coloured, after.find("raw").unwrap()),
            Token::Plain
        );
        assert_eq!(coloured.last_read(), after.len());
        // A fence closed, a fence removed, a page in a fence.
        changed("a.md", notes, notes.len()..notes.len(), "```");
        changed("a.md", notes, 4..12, "");
        changed(
            "a.md",
            notes,
            7..11,
            "html\n<script>let a = /b/;</script>\n<i\n",
        );
    }

    /// Numbers that look drawn at random, the same on every run.
    struct Dice(u64);

    impl Dice {
        /// A number below `limit`.
        fn below(&mut self, limit: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as usize % limit
        }
    }

    /// The char boundary of `text` at or before byte `at`.
    fn boundary(text: &str, mut at: usize) -> usize {
        at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    const CODE: &str = r##"fn main<'a>(x: &'a str) -> Vec<u8> {
    let r = r#"raw "text"
over lines"#; let c = 'x'; let n = b'\n';
    let total = compute(alpha, 42, 1.5e3) / 2; // done
    const re = /ab+c[/]/gi, d = a / b;
    println!("{}", total);
}
"##;

    const PAGE: &str = r#"<!DOCTYPE html>
<!-- a note
over lines -->
<html lang="en">
<head>
<style>
body { color: #fff; /* white
 text */ }
</style>
<script src="a.js"/>
<script type="module">
let x = `a
${b}` + 'y'; // note
const re = /a+/g;
</SCRIPT>
</head>
<body class="main
 wide" id='top'>
<a href="u">text é</a>
</body>
</html>
"#;

    const NOTES: &str = r#"# Title

Some `code` here and `more`.
> a quote

```rust
fn a<'a>() { let s = "x
y"; /* c
*/ }
```
text
```html
<p class="a">
<script>let x = 1;</script>
```
```md
# inner
> q
```
```
raw
```
  ```js
let y = `t
`;
"#;

    /// A text in `language`, with all that it can leave open over lines.
    fn sample(language: &Language) -> String {
        let mut text = match language.family {
            Family::Markup => return PAGE.to_string(),
            Family::Markdown => return NOTES.to_string(),
            Family::Text => return "plain words\nand \"more\" of them\n".to_string(),
            Family::Code => CODE.to_string(),
        };
        for prefix in language.line_comments {
            text += &format!("call(1) {prefix} a note\n");
        }
        if let Some((open, close)) = language.block_comment {
            text += &format!("a {open} over\ntwo lines {close} b = 2\n");
        }
        for quote in language.quotes.iter().map(|quote| *quote as char) {
            text += &format!("s = {quote}one\\{quote} two{quote} + {quote}left open\n");
            text += &format!("t = {quote}goes \\\non{quote}\n");
            if language.multiline.contains(&(quote as u8)) {
                text += &format!("m = {quote}over\nlines{quote};\n");
            }
            if language.triple_quotes {
                text += &format!(
                    "d = {quote}{quote}{quote}doc {quote} over\nlines{quote}{quote}{quote} + 1\n"
                );
            }
        }
        for keyword in language.keywords.iter().take(6) {
            text += &format!("{keyword} name(x) {{ Type }}\n");
        }
        text
    }

    /// What is typed into a text of `language`: its own delimiters among those of all.
    fn pieces(language: &Language) -> Vec<String> {
        let all = [
            "\n",
            "\n",
            "\n",
            "\r\n",
            " ",
            "\\",
            "/",
            "'",
            "\"",
            "`",
            "#",
            "x",
            "fn",
            "A",
            "1",
            "(",
            "=",
            ",",
            "é",
            "日",
            "r#\"",
            "\"#",
            "br\"",
            "\"\"\"",
            "'''",
            "/*",
            "*/",
            "//",
            "--",
            "<!--",
            "-->",
            "<script>",
            "</script>",
            "<style",
            "</STYLE>",
            "<a href=\"u\"",
            ">",
            "/>",
            "<",
            "```",
            "```rust\n",
            "```html\n",
            "```md\n",
            "\n```\n",
            " ```",
            "# ",
            "> ",
            "'a",
            "'\n'",
            ".5",
            "<<EOF",
            "EOF\n",
            "[",
            "]",
        ];
        let mut pieces: Vec<String> = all.iter().map(|piece| piece.to_string()).collect();
        pieces.extend(
            language
                .line_comments
                .iter()
                .map(|prefix| prefix.to_string()),
        );
        if let Some((open, close)) = language.block_comment {
            pieces.extend([open.to_string(), close.to_string()]);
        }
        for quote in language.quotes.iter().map(|quote| *quote as char) {
            pieces.extend([quote.to_string(), quote.to_string().repeat(3)]);
        }
        pieces.extend(language.keywords.first().map(|keyword| keyword.to_string()));
        pieces
    }

    #[test]
    fn every_language_follows_thousands_of_changes() {
        for (number, language) in languages().enumerate() {
            let mut dice = Dice(number as u64 + 1);
            let pieces = pieces(language);
            let mut text = sample(language);
            let mut coloured = Coloured::new(&text, language);
            for _ in 0..2500 {
                let at = boundary(&text, dice.below(text.len() + 1));
                // Mostly a few bytes go; now and then lines do, and more when the text
                // has grown long.
                let gone = match dice.below(12) {
                    0..4 => 0,
                    4..11 if text.len() < 1500 => dice.below(6),
                    4..11 => dice.below(200),
                    _ => dice.below(80),
                };
                let end = boundary(&text, at + gone);
                let written: String = (0..dice.below(4))
                    .map(|_| pieces[dice.below(pieces.len())].as_str())
                    .collect();
                change(&mut coloured, &mut text, language, at..end, &written);
            }
        }
    }

    /// A source of `lines` lines.
    fn big(lines: usize) -> String {
        "    let total = compute(alpha, \"text\", 42); // a note\n".repeat(lines)
    }

    /// Type `typed` at the byte `at` of `text`, have `coloured` follow: the bytes read.
    fn typed(coloured: &mut Coloured, text: &mut String, at: usize, typed: &str) -> usize {
        text.insert_str(at, typed);
        let language = Language::of(Path::new("a.rs"));
        coloured.edit(text, language, at..at, at..at + typed.len());
        coloured.last_read()
    }

    #[test]
    fn a_change_in_a_big_file_reads_a_line_or_so() {
        let language = Language::of(Path::new("a.rs"));
        let mut text = big(200_000);
        let line = text.len() / 200_000;
        let mut coloured = Coloured::new(&text, language);
        assert_eq!(coloured.last_read(), text.len());
        // A char typed in a line in the middle of the file: its line is read.
        let here = 100_000 * line;
        assert_eq!(typed(&mut coloured, &mut text, here + 8, "x"), line + 1);
        // A line end: the line cut, and the one it makes.
        assert_eq!(typed(&mut coloured, &mut text, here + 8, "\n"), line + 2);
        // A block comment opened there takes the rest of the file, read to its end.
        let read = typed(&mut coloured, &mut text, here + 12, "/*");
        assert_eq!(read, text.len() - (here + 9));
        assert_eq!(
            coloured.spans().last().map(|span| span.token),
            Some(Token::Comment)
        );
        // Typed in the comment, there and far below: a line each time.
        assert_eq!(typed(&mut coloured, &mut text, here + 14, "y"), line - 4);
        assert_eq!(
            typed(&mut coloured, &mut text, here + 50_000 * line, "z"),
            line + 1
        );
        // Closed, it gives back all it took: every line below changes colour, and is read.
        let read = typed(&mut coloured, &mut text, here + 15, "*/");
        assert_eq!(read, text.len() - (here + 9));
        // After which a char is a line again.
        assert_eq!(typed(&mut coloured, &mut text, here + 17, "w"), line - 1);
        assert_eq!(coloured.spans(), highlight(&text, language));
    }

    #[test]
    #[ignore = "a measure, not a check"]
    fn colouring_again_is_measured() {
        let language = Language::of(Path::new("a.rs"));
        let mut text = big(200_000);
        let timed = |name: &str, work: &mut dyn FnMut()| {
            let start = std::time::Instant::now();
            work();
            println!("{name}: {:?}", start.elapsed());
        };
        println!("{} lines, {} bytes", 200_000, text.len());
        timed("highlight", &mut || {
            std::hint::black_box(highlight(&text, language));
        });
        timed("Coloured::new", &mut || {
            std::hint::black_box(Coloured::new(&text, language));
        });
        let mut coloured = Coloured::new(&text, language);
        println!("{} spans", coloured.spans().len());
        let middle = text.len() / 2 + 8;
        for (after, typed) in [(0, "x"), (0, "y"), (0, "\n"), (1, "/*"), (3, "*/")] {
            let at = middle + after;
            text.insert_str(at, typed);
            timed(&format!("edit, {typed:?} typed in the middle"), &mut || {
                coloured.edit(&text, language, at..at, at..at + typed.len());
            });
            println!("  {} bytes read", coloured.last_read());
        }
        let last = text.len() - 3;
        text.insert(last, 'x');
        timed("edit, a char typed on the last line", &mut || {
            coloured.edit(&text, language, last..last, last..last + 1);
        });
        text.insert(8, 'x');
        timed("edit, a char typed on the first line", &mut || {
            coloured.edit(&text, language, 8..8, 8..9);
        });
        assert_eq!(coloured.spans(), highlight(&text, language));
    }
}
