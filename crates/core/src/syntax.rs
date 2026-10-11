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

/// Colour `text`: spans in order, covering it entirely.
pub fn highlight(text: &str, language: &Language) -> Vec<Span> {
    let mut spans = Spans(Vec::new());
    match language.family {
        Family::Text => {}
        Family::Code => code(text, language, &mut spans),
        Family::Markup => markup(text, &mut spans),
        Family::Markdown => markdown(text, &mut spans),
    }
    spans.push(text.len(), Token::Plain);
    spans.0
}

fn is_word_byte(byte: u8) -> bool {
    // Bytes of non-ASCII letters count as word bytes, which also keeps offsets on char bounds.
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn find_from(text: &str, from: usize, needle: &str) -> Option<usize> {
    text.get(from..)?.find(needle).map(|index| from + index)
}

/// End of the string opened by `quote` at byte `start`.
fn string_end(text: &str, start: usize, quote: u8, language: &Language) -> usize {
    let bytes = text.as_bytes();
    let triple = [quote; 3];
    if language.triple_quotes && bytes[start..].starts_with(&triple) {
        let closing = std::str::from_utf8(&triple).unwrap_or("\"\"\"");
        return find_from(text, start + 3, closing).map_or(text.len(), |index| index + 3);
    }
    let multiline = language.multiline.contains(&quote);
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            byte if byte == quote => return index + 1,
            b'\n' if !multiline => return index,
            _ => {}
        }
        index += 1;
    }
    text.len()
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

/// End of the Rust raw string (`r"…"`, `r#"…"#`, `br"…"`) starting at byte `index`.
fn raw_string_end(text: &str, index: usize) -> Option<usize> {
    let rest = &text[index..];
    let rest = rest.strip_prefix('b').unwrap_or(rest).strip_prefix('r')?;
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    let body = rest[hashes..].strip_prefix('"')?;
    let closing = format!("\"{}", "#".repeat(hashes));
    let start = text.len() - body.len();
    Some(find_from(text, start, &closing).map_or(text.len(), |found| found + closing.len()))
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

fn code(text: &str, language: &Language, spans: &mut Spans) {
    let bytes = text.as_bytes();
    let mut index = 0;
    // The last byte of code seen: after an operator a `/` opens a regular expression,
    // after a value it divides.
    let mut last = b'(';
    while index < bytes.len() {
        let byte = bytes[index];
        let rest = &text[index..];
        let lifetime = language.rust && byte == b'\'' && !is_char_literal(rest);
        let (end, token) = if language
            .line_comments
            .iter()
            .any(|prefix| rest.starts_with(prefix))
        {
            let end = find_from(text, index, "\n").unwrap_or(text.len());
            (end, Token::Comment)
        } else if let Some((open, close)) = language.block_comment
            && rest.starts_with(open)
        {
            let end = find_from(text, index + open.len(), close)
                .map_or(text.len(), |found| found + close.len());
            (end, Token::Comment)
        } else if language.rust
            && let Some(end) = raw_string_end(text, index)
        {
            (end, Token::String)
        } else if language.regex_literals
            && byte == b'/'
            && b"(,=:[!&|?{};".contains(&last)
            && let Some(end) = regex_end(bytes, index)
        {
            (end, Token::String)
        } else if language.quotes.contains(&byte) && !lifetime {
            let end = string_end(text, index, byte, language).min(text.len());
            (end, Token::String)
        } else if byte.is_ascii_digit() {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || bytes[end] == b'_'
                    || (bytes[end] == b'.' && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)))
            {
                end += 1;
            }
            (end, Token::Number)
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
            (end, token)
        } else {
            if !byte.is_ascii_whitespace() {
                last = byte;
            }
            index += 1;
            continue;
        };
        if token != Token::Comment {
            // A keyword (`return`) is followed by a value, as an operator is.
            last = if token == Token::Keyword {
                b'('
            } else {
                bytes[end - 1]
            };
        }
        spans.mark(index, end, token);
        index = end;
    }
}

/// Colour the bytes `from..to` of `text` as `language`: a script in a page, a fenced block.
fn embed(text: &str, from: usize, to: usize, language: &Language, spans: &mut Spans) {
    if from >= to {
        return;
    }
    if language.family == Family::Text {
        spans.mark(from, to, Token::String);
        return;
    }
    spans.push(from, Token::Plain);
    for span in highlight(&text[from..to], language) {
        spans.push(from + span.end, span.token);
    }
}

/// HTML / XML: comments, tag names, attribute names and values; scripts and styles in
/// their own language.
fn markup(text: &str, spans: &mut Spans) {
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(open) = find_from(text, index, "<") {
        if text[open..].starts_with("<!--") {
            let end = find_from(text, open + 4, "-->").map_or(text.len(), |found| found + 3);
            spans.mark(open, end, Token::Comment);
            index = end;
            continue;
        }
        let close = find_from(text, open, ">").unwrap_or(text.len());
        let mut at = open + 1;
        let mut named = false;
        while at < close {
            let byte = bytes[at];
            if byte == b'"' || byte == b'\'' {
                let end = text[at + 1..close]
                    .find(byte as char)
                    .map_or(close, |found| at + 1 + found + 1);
                spans.mark(at, end, Token::String);
                at = end;
            } else if is_word_byte(byte) {
                let mut end = at + 1;
                while end < close && (is_word_byte(bytes[end]) || b"-:.".contains(&bytes[end])) {
                    end += 1;
                }
                let token = if named { Token::Type } else { Token::Keyword };
                spans.mark(at, end, token);
                named = true;
                at = end;
            } else {
                at += 1;
            }
        }
        index = (close + 1).min(text.len());
        let name: String = text[open + 1..close]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        let language = match name.as_str() {
            "script" => Language::named("JavaScript"),
            "style" => Language::named("CSS"),
            _ => None,
        };
        if let Some(language) = language.filter(|_| !text[open..close].ends_with('/')) {
            let closing = format!("</{name}");
            let end = text[index..]
                .to_ascii_lowercase()
                .find(&closing)
                .map_or(text.len(), |at| index + at);
            embed(text, index, end, language, spans);
            index = end;
        }
    }
}

/// Markdown: headings, quotes, fenced and inline code.
fn markdown(text: &str, spans: &mut Spans) {
    let mut start = 0;
    // The fence being read: where its code starts, and the language it names.
    let mut fence: Option<(usize, &Language)> = None;
    for line in text.split('\n') {
        let end = start + line.len();
        let trimmed = line.trim_start();
        if let Some(named) = trimmed.strip_prefix("```") {
            match fence.take() {
                Some((body, language)) => embed(text, body, start, language, spans),
                None => fence = Some(((end + 1).min(text.len()), Language::fenced(named))),
            }
            spans.mark(start, end, Token::String);
        } else if fence.is_some() {
            // Coloured as a whole when the fence closes.
        } else if trimmed.starts_with('#') {
            spans.mark(start, end, Token::Keyword);
        } else if trimmed.starts_with('>') {
            spans.mark(start, end, Token::Comment);
        } else {
            let mut from = start;
            while let Some(open) = find_from(text, from, "`").filter(|open| *open < end) {
                let Some(close) = find_from(text, open + 1, "`").filter(|close| *close < end)
                else {
                    break;
                };
                spans.mark(open, close + 1, Token::String);
                from = close + 1;
            }
        }
        start = end + 1;
    }
    if let Some((body, language)) = fence {
        embed(text, body, text.len(), language, spans);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
