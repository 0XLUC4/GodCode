//! Completion of the word being typed: snippets, keywords and words of the file
//! (`complete`), or all there is to offer, found by its letters (`suggest`).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::byte_index;
use crate::syntax::Language;

const MAX_COMPLETIONS: usize = 30;
/// Shorter words are quicker typed than picked.
const MIN_WORD_CHARS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Snippet,
    Keyword,
    Word,
    /// A name the language server knows: a method, a field, a type.
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// What is inserted. In snippets a tab is one indent level and `$0` where the caret ends.
    pub body: String,
    pub kind: Kind,
}

type Snippets = &'static [(&'static str, &'static str)];

const C_LIKE: Snippets = &[
    ("for", "for (int i = 0; i < $0; i++) {\n\t\n}"),
    ("if", "if ($0) {\n\t\n}"),
    ("ifelse", "if ($0) {\n\t\n} else {\n\t\n}"),
    ("while", "while ($0) {\n\t\n}"),
    ("dowhile", "do {\n\t\n} while ($0);"),
    (
        "switch",
        "switch ($0) {\n\tcase :\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}",
    ),
];

/// What TypeScript has that JavaScript has not.
const TYPED: Snippets = &[
    ("interface", "interface $0 {\n\t\n}"),
    ("type", "type $0 = ;"),
    ("enum", "enum $0 {\n\t\n}"),
];

const SCRIPT: Snippets = &[
    ("function", "function $0() {\n\t\n}"),
    ("for", "for (let i = 0; i < $0; i++) {\n\t\n}"),
    ("forof", "for (const item of $0) {\n\t\n}"),
    ("if", "if ($0) {\n\t\n}"),
    ("class", "class $0 {\n\tconstructor() {\n\t\t\n\t}\n}"),
    ("log", "console.log($0);"),
    ("try", "try {\n\t$0\n} catch (error) {\n\t\n}"),
    ("main", "function main() {\n\t$0\n}\n\nmain();"),
    ("arrow", "const $0 = () => {\n\t\n};"),
    ("async", "async function $0() {\n\t\n}"),
    ("ifelse", "if ($0) {\n\t\n} else {\n\t\n}"),
    ("while", "while ($0) {\n\t\n}"),
    (
        "switch",
        "switch ($0) {\n\tcase :\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}",
    ),
    ("forin", "for (const key in $0) {\n\t\n}"),
    ("foreach", "$0.forEach((item) => {\n\t\n});"),
    ("import", "import { $0 } from \"\";"),
    ("export", "export function $0() {\n\t\n}"),
    ("promise", "new Promise((resolve, reject) => {\n\t$0\n});"),
    (
        "fetch",
        "const response = await fetch($0);\nconst data = await response.json();",
    ),
    ("timeout", "setTimeout(() => {\n\t$0\n}, 1000);"),
];

/// Snippets by language name, with one place for the caret: what `complete` offers.
const SNIPPETS: &[(&str, Snippets)] = &[
    (
        "Rust",
        &[
            ("fn", "fn $0() {\n\t\n}"),
            ("for", "for $0 in  {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("impl", "impl $0 {\n\t\n}"),
            ("struct", "struct $0 {\n\t\n}"),
            ("enum", "enum $0 {\n\t\n}"),
            ("match", "match $0 {\n\t_ => {}\n}"),
            ("println", "println!(\"$0\");"),
            ("test", "#[test]\nfn $0() {\n\t\n}"),
            ("main", "fn main() {\n\t$0\n}"),
            ("pubfn", "pub fn $0() {\n\t\n}"),
            ("while", "while $0 {\n\t\n}"),
            ("loop", "loop {\n\t$0\n}"),
            ("iflet", "if let Some($0) =  {\n\t\n}"),
            ("ifelse", "if $0 {\n\t\n} else {\n\t\n}"),
            ("trait", "trait $0 {\n\t\n}"),
            ("mod", "mod $0 {\n\t\n}"),
            (
                "tests",
                "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn $0() {\n\t\t\n\t}\n}",
            ),
            (
                "derive",
                "#[derive(Debug, Clone, PartialEq)]\nstruct $0 {\n\t\n}",
            ),
            ("vec", "vec![$0]"),
            ("dbg", "dbg!($0);"),
        ],
    ),
    (
        "Python",
        &[
            ("def", "def $0():\n\tpass"),
            ("class", "class $0:\n\tdef __init__(self):\n\t\tpass"),
            ("for", "for $0 in :\n\tpass"),
            ("if", "if $0:\n\tpass"),
            ("ifmain", "if __name__ == \"__main__\":\n\t$0"),
            ("print", "print($0)"),
            ("try", "try:\n\t$0\nexcept Exception as error:\n\tpass"),
            (
                "main",
                "def main():\n\t$0\n\n\nif __name__ == \"__main__\":\n\tmain()",
            ),
            ("while", "while $0:\n\tpass"),
            ("ifelse", "if $0:\n\tpass\nelse:\n\tpass"),
            ("with", "with $0 as :\n\tpass"),
            ("async", "async def $0():\n\tpass"),
            ("init", "def __init__(self$0):\n\tpass"),
            ("dataclass", "@dataclass\nclass $0:\n\tpass"),
            ("lambda", "lambda $0: "),
        ],
    ),
    ("JavaScript", SCRIPT),
    ("TypeScript", SCRIPT),
    ("TypeScript", TYPED),
    (
        "Go",
        &[
            ("func", "func $0() {\n\t\n}"),
            ("for", "for $0 {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("iferr", "if err != nil {\n\treturn $0\n}"),
            (
                "main",
                "package main\n\nimport \"fmt\"\n\nfunc main() {\n\t$0\n}",
            ),
            ("struct", "type $0 struct {\n\t\n}"),
            ("interface", "type $0 interface {\n\t\n}"),
            ("method", "func (r *Receiver) $0() {\n\t\n}"),
            ("forrange", "for i, item := range $0 {\n\t\n}"),
            ("switch", "switch $0 {\ncase :\n\t\ndefault:\n\t\n}"),
            ("println", "fmt.Println($0)"),
            ("test", "func Test$0(t *testing.T) {\n\t\n}"),
        ],
    ),
    (
        "HTML",
        &[(
            "html5",
            "<!doctype html>\n<html lang=\"en\">\n<head>\n\t<meta charset=\"utf-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\t<title>$0</title>\n</head>\n<body>\n\t\n</body>\n</html>",
        )],
    ),
    ("C", C_LIKE),
    (
        "C",
        &[
            (
                "main",
                "int main(int argc, char *argv[]) {\n\t$0\n\treturn 0;\n}",
            ),
            ("include", "#include <$0>"),
            ("struct", "struct $0 {\n\t\n};"),
            ("printf", "printf(\"$0\\n\");"),
        ],
    ),
    ("C++", C_LIKE),
    (
        "C++",
        &[
            ("main", "int main() {\n\t$0\n\treturn 0;\n}"),
            ("include", "#include <$0>"),
            ("class", "class $0 {\npublic:\n\t\n};"),
            ("struct", "struct $0 {\n\t\n};"),
            ("forrange", "for (const auto& item : $0) {\n\t\n}"),
            ("cout", "std::cout << $0 << std::endl;"),
        ],
    ),
    ("Java", C_LIKE),
    (
        "Java",
        &[
            ("main", "public static void main(String[] args) {\n\t$0\n}"),
            ("class", "public class $0 {\n\t\n}"),
            ("foreach", "for (var item : $0) {\n\t\n}"),
            ("sout", "System.out.println($0);"),
            ("try", "try {\n\t$0\n} catch (Exception e) {\n\t\n}"),
        ],
    ),
    ("C#", C_LIKE),
    (
        "C#",
        &[
            ("main", "static void Main(string[] args)\n{\n\t$0\n}"),
            ("class", "public class $0\n{\n\t\n}"),
            ("foreach", "foreach (var item in $0)\n{\n\t\n}"),
            ("prop", "public string $0 { get; set; }"),
            ("cw", "Console.WriteLine($0);"),
            ("try", "try\n{\n\t$0\n}\ncatch (Exception e)\n{\n\t\n}"),
        ],
    ),
    (
        "Kotlin",
        &[
            ("main", "fun main() {\n\t$0\n}"),
            ("fun", "fun $0() {\n\t\n}"),
            ("class", "class $0 {\n\t\n}"),
            ("for", "for (item in $0) {\n\t\n}"),
            ("if", "if ($0) {\n\t\n}"),
            ("when", "when ($0) {\n\telse -> {}\n}"),
            ("println", "println($0)"),
        ],
    ),
    (
        "Swift",
        &[
            ("func", "func $0() {\n\t\n}"),
            ("struct", "struct $0 {\n\t\n}"),
            ("class", "class $0 {\n\t\n}"),
            ("for", "for item in $0 {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("guard", "guard $0 else {\n\treturn\n}"),
            ("print", "print($0)"),
        ],
    ),
    (
        "PHP",
        &[
            ("function", "function $0() {\n\t\n}"),
            ("class", "class $0 {\n\t\n}"),
            ("foreach", "foreach ($0 as $item) {\n\t\n}"),
            ("if", "if ($0) {\n\t\n}"),
            ("echo", "echo $0;"),
        ],
    ),
    (
        "Ruby",
        &[
            ("def", "def $0\n\t\nend"),
            ("class", "class $0\n\t\nend"),
            ("each", "$0.each do |item|\n\t\nend"),
            ("if", "if $0\n\t\nend"),
            ("puts", "puts $0"),
        ],
    ),
    (
        "Shell",
        &[
            ("if", "if [ $0 ]; then\n\t\nfi"),
            ("for", "for item in $0; do\n\t\ndone"),
            ("while", "while $0; do\n\t\ndone"),
            ("function", "$0() {\n\t\n}"),
            ("case", "case $0 in\n\t*)\n\t\t;;\nesac"),
        ],
    ),
    (
        "SQL",
        &[
            ("select", "SELECT $0 FROM ;"),
            ("insert", "INSERT INTO $0 () VALUES ();"),
            ("update", "UPDATE $0 SET  WHERE ;"),
            ("create", "CREATE TABLE $0 (\n\tid INTEGER PRIMARY KEY\n);"),
        ],
    ),
];

/// The snippets that come with `language`.
fn snippets_of(language: &str) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let own = SNIPPETS.iter().filter(move |(name, _)| *name == language);
    own.flat_map(|(_, snippets)| snippets.iter())
}

const STOPS_C_LIKE: Snippets = &[
    (
        "for",
        "for (int ${1:i} = 0; ${1:i} < ${2:count}; ${1:i}++) {\n\t$0\n}",
    ),
    ("if", "if (${1:condition}) {\n\t$0\n}"),
    ("ifelse", "if (${1:condition}) {\n\t$2\n} else {\n\t$0\n}"),
    ("while", "while (${1:condition}) {\n\t$0\n}"),
    ("dowhile", "do {\n\t$0\n} while (${1:condition});"),
    (
        "switch",
        "switch (${1:value}) {\n\tcase ${2:0}:\n\t\t$0\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}",
    ),
];

const STOPS_TYPED: Snippets = &[
    ("interface", "interface ${1:Name} {\n\t$0\n}"),
    ("type", "type ${1:Name} = ${2:unknown};"),
    ("enum", "enum ${1:Name} {\n\t$0\n}"),
    ("exportinterface", "export interface ${1:Name} {\n\t$0\n}"),
    ("exporttype", "export type ${1:Name} = ${2:unknown};"),
];

const STOPS_SCRIPT: Snippets = &[
    ("function", "function ${1:name}($2) {\n\t$0\n}"),
    (
        "for",
        "for (let ${1:i} = 0; ${1:i} < ${2:count}; ${1:i}++) {\n\t$0\n}",
    ),
    ("forof", "for (const ${1:item} of ${2:items}) {\n\t$0\n}"),
    ("if", "if (${1:condition}) {\n\t$0\n}"),
    (
        "class",
        "class ${1:Name} {\n\tconstructor($2) {\n\t\t$0\n\t}\n}",
    ),
    ("log", "console.log($0);"),
    ("try", "try {\n\t$1\n} catch (${2:error}) {\n\t$0\n}"),
    ("main", "function main() {\n\t$0\n}\n\nmain();"),
    ("arrow", "const ${1:name} = ($2) => {\n\t$0\n};"),
    ("async", "async function ${1:name}($2) {\n\t$0\n}"),
    ("ifelse", "if (${1:condition}) {\n\t$2\n} else {\n\t$0\n}"),
    ("while", "while (${1:condition}) {\n\t$0\n}"),
    (
        "switch",
        "switch (${1:value}) {\n\tcase ${2:0}:\n\t\t$0\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}",
    ),
    ("forin", "for (const ${1:key} in ${2:object}) {\n\t$0\n}"),
    ("foreach", "${1:items}.forEach((${2:item}) => {\n\t$0\n});"),
    ("import", "import { $2 } from \"${1:module}\";"),
    ("export", "export function ${1:name}($2) {\n\t$0\n}"),
    ("promise", "new Promise((resolve, reject) => {\n\t$0\n});"),
    (
        "fetch",
        "const ${1:response} = await fetch(${2:url});\nconst ${3:data} = await ${1:response}.json();",
    ),
    ("timeout", "setTimeout(() => {\n\t$0\n}, ${1:1000});"),
    (
        "ifelseif",
        "if (${1:condition}) {\n\t$2\n} else if (${3:other}) {\n\t$4\n} else {\n\t$0\n}",
    ),
    (
        "tryfinally",
        "try {\n\t$1\n} catch (${2:error}) {\n\t$3\n} finally {\n\t$0\n}",
    ),
    ("asyncarrow", "const ${1:name} = async ($2) => {\n\t$0\n};"),
    (
        "exportdefault",
        "export default function ${1:name}($2) {\n\t$0\n}",
    ),
    ("exportconst", "export const ${1:name} = ${2:value};"),
    ("importdefault", "import ${2:name} from \"${1:module}\";"),
    ("importall", "import * as ${2:name} from \"${1:module}\";"),
    ("require", "const ${2:name} = require(\"${1:module}\");"),
    ("map", "${1:items}.map((${2:item}) => ${3:item})"),
    ("filter", "${1:items}.filter((${2:item}) => ${3:item})"),
    ("interval", "setInterval(() => {\n\t$0\n}, ${1:1000});"),
    (
        "listener",
        "${1:element}.addEventListener(\"${2:click}\", (${3:event}) => {\n\t$0\n});",
    ),
];

/// The snippets with tab stops, by language name: all those of `SNIPPETS`, to be walked
/// with Tab, and more. `suggest` offers these.
const STOPS_SNIPPETS: &[(&str, Snippets)] = &[
    (
        "Rust",
        &[
            ("fn", "fn ${1:name}($2) {\n\t$0\n}"),
            ("for", "for ${1:item} in ${2:items} {\n\t$0\n}"),
            ("if", "if ${1:condition} {\n\t$0\n}"),
            ("impl", "impl ${1:Type} {\n\t$0\n}"),
            ("struct", "struct ${1:Name} {\n\t$0\n}"),
            ("enum", "enum ${1:Name} {\n\t$0\n}"),
            ("match", "match ${1:value} {\n\t${2:_} => {$0}\n}"),
            ("println", "println!(\"$0\");"),
            ("test", "#[test]\nfn ${1:name}() {\n\t$0\n}"),
            ("main", "fn main() {\n\t$0\n}"),
            ("pubfn", "pub fn ${1:name}($2) {\n\t$0\n}"),
            ("while", "while ${1:condition} {\n\t$0\n}"),
            ("loop", "loop {\n\t$0\n}"),
            ("iflet", "if let ${1:Some(value)} = ${2:option} {\n\t$0\n}"),
            ("ifelse", "if ${1:condition} {\n\t$2\n} else {\n\t$0\n}"),
            ("trait", "trait ${1:Name} {\n\t$0\n}"),
            ("mod", "mod ${1:name} {\n\t$0\n}"),
            (
                "tests",
                "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn ${1:name}() {\n\t\t$0\n\t}\n}",
            ),
            (
                "derive",
                "#[derive(${1:Debug, Clone, PartialEq})]\nstruct ${2:Name} {\n\t$0\n}",
            ),
            ("vec", "vec![$0]"),
            ("dbg", "dbg!($0);"),
            ("impltrait", "impl ${1:Trait} for ${2:Type} {\n\t$0\n}"),
            ("pubstruct", "pub struct ${1:Name} {\n\t$0\n}"),
            ("pubenum", "pub enum ${1:Name} {\n\t$0\n}"),
            ("asyncfn", "async fn ${1:name}($2) {\n\t$0\n}"),
            (
                "fnresult",
                "fn ${1:name}($2) -> Result<${3:()}, ${4:Error}> {\n\t$0\n}",
            ),
            ("new", "pub fn new($1) -> Self {\n\tSelf {$0}\n}"),
            ("attrderive", "#[derive(${1:Debug, Clone, PartialEq})]"),
            (
                "matchoption",
                "match ${1:option} {\n\tSome(${2:value}) => {$3}\n\tNone => {$0}\n}",
            ),
            (
                "matchresult",
                "match ${1:result} {\n\tOk(${2:value}) => {$3}\n\tErr(${4:error}) => {$0}\n}",
            ),
            (
                "letelse",
                "let Some(${1:value}) = ${2:option} else {\n\t${0:return};\n};",
            ),
            (
                "whilelet",
                "while let Some(${1:item}) = ${2:items}.next() {\n\t$0\n}",
            ),
            ("closure", "|${1:item}| ${0:item}"),
            (
                "default",
                "impl Default for ${1:Type} {\n\tfn default() -> Self {\n\t\t$0\n\t}\n}",
            ),
            (
                "display",
                "impl std::fmt::Display for ${1:Type} {\n\tfn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n\t\twrite!(f, \"$0\")\n\t}\n}",
            ),
            (
                "from",
                "impl From<${1:Other}> for ${2:Type} {\n\tfn from(${3:other}: ${1:Other}) -> Self {\n\t\t$0\n\t}\n}",
            ),
        ],
    ),
    (
        "Python",
        &[
            ("def", "def ${1:name}($2):\n\t${0:pass}"),
            (
                "class",
                "class ${1:Name}:\n\tdef __init__(self$2):\n\t\t${0:pass}",
            ),
            ("for", "for ${1:item} in ${2:items}:\n\t${0:pass}"),
            ("if", "if ${1:condition}:\n\t${0:pass}"),
            ("ifmain", "if __name__ == \"__main__\":\n\t${0:main()}"),
            ("print", "print($0)"),
            (
                "try",
                "try:\n\t${1:pass}\nexcept ${2:Exception} as ${3:error}:\n\t${0:pass}",
            ),
            (
                "main",
                "def main():\n\t${0:pass}\n\n\nif __name__ == \"__main__\":\n\tmain()",
            ),
            ("while", "while ${1:condition}:\n\t${0:pass}"),
            (
                "ifelse",
                "if ${1:condition}:\n\t${2:pass}\nelse:\n\t${0:pass}",
            ),
            ("with", "with ${1:expression} as ${2:name}:\n\t${0:pass}"),
            ("async", "async def ${1:name}($2):\n\t${0:pass}"),
            ("init", "def __init__(self$1):\n\t${0:pass}"),
            ("dataclass", "@dataclass\nclass ${1:Name}:\n\t${0:pass}"),
            ("lambda", "lambda ${1:item}: ${0:item}"),
            (
                "ifelif",
                "if ${1:condition}:\n\t${2:pass}\nelif ${3:other}:\n\t${4:pass}\nelse:\n\t${0:pass}",
            ),
            (
                "tryfinally",
                "try:\n\t${1:pass}\nexcept ${2:Exception} as ${3:error}:\n\t${4:pass}\nfinally:\n\t${0:pass}",
            ),
            ("listcomp", "[${1:item} for ${1:item} in ${2:items}]"),
            (
                "dictcomp",
                "{${1:key}: ${2:value} for ${1:key}, ${2:value} in ${3:items}}",
            ),
            (
                "property",
                "@property\ndef ${1:name}(self):\n\treturn self._${1:name}",
            ),
            ("method", "def ${1:name}(self$2):\n\t${0:pass}"),
            (
                "classmethod",
                "@classmethod\ndef ${1:name}(cls$2):\n\t${0:pass}",
            ),
            (
                "staticmethod",
                "@staticmethod\ndef ${1:name}($2):\n\t${0:pass}",
            ),
            (
                "withopen",
                "with open(${1:path}) as ${2:file}:\n\t${0:pass}",
            ),
            ("fromimport", "from ${1:module} import ${2:name}"),
            (
                "enumerate",
                "for ${1:index}, ${2:item} in enumerate(${3:items}):\n\t${0:pass}",
            ),
            ("test", "def test_${1:name}():\n\t${0:pass}"),
        ],
    ),
    ("JavaScript", STOPS_SCRIPT),
    ("TypeScript", STOPS_SCRIPT),
    ("TypeScript", STOPS_TYPED),
    (
        "Go",
        &[
            ("func", "func ${1:name}($2) {\n\t$0\n}"),
            ("for", "for ${1:condition} {\n\t$0\n}"),
            ("if", "if ${1:condition} {\n\t$0\n}"),
            ("iferr", "if err != nil {\n\treturn ${1:err}\n}"),
            (
                "main",
                "package main\n\nimport \"fmt\"\n\nfunc main() {\n\t$0\n}",
            ),
            ("struct", "type ${1:Name} struct {\n\t$0\n}"),
            ("interface", "type ${1:Name} interface {\n\t$0\n}"),
            (
                "method",
                "func (${1:r} *${2:Receiver}) ${3:name}($4) {\n\t$0\n}",
            ),
            (
                "forrange",
                "for ${1:i}, ${2:item} := range ${3:items} {\n\t$0\n}",
            ),
            (
                "switch",
                "switch ${1:value} {\ncase ${2:0}:\n\t$0\ndefault:\n}",
            ),
            ("println", "fmt.Println($0)"),
            ("test", "func Test${1:Name}(t *testing.T) {\n\t$0\n}"),
            (
                "fori",
                "for ${1:i} := 0; ${1:i} < ${2:count}; ${1:i}++ {\n\t$0\n}",
            ),
            ("ifelse", "if ${1:condition} {\n\t$2\n} else {\n\t$0\n}"),
            (
                "iferrwrap",
                "if err != nil {\n\treturn fmt.Errorf(\"${1:context}: %w\", err)\n}",
            ),
            ("go", "go func() {\n\t$0\n}()"),
            ("defer", "defer func() {\n\t$0\n}()"),
            (
                "select",
                "select {\ncase ${1:value} := <-${2:channel}:\n\t$0\n}",
            ),
            (
                "handler",
                "func ${1:handler}(w http.ResponseWriter, r *http.Request) {\n\t$0\n}",
            ),
        ],
    ),
    (
        "HTML",
        &[(
            "html5",
            "<!doctype html>\n<html lang=\"${1:en}\">\n<head>\n\t<meta charset=\"utf-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\t<title>${2:Title}</title>\n</head>\n<body>\n\t$0\n</body>\n</html>",
        )],
    ),
    ("C", STOPS_C_LIKE),
    (
        "C",
        &[
            (
                "main",
                "int main(int argc, char *argv[]) {\n\t$0\n\treturn 0;\n}",
            ),
            ("include", "#include <${1:stdio.h}>"),
            ("struct", "struct ${1:Name} {\n\t$0\n};"),
            ("printf", "printf(\"${1:text}\\n\"$0);"),
            ("function", "${1:void} ${2:name}(${3:void}) {\n\t$0\n}"),
            ("typedef", "typedef struct {\n\t$0\n} ${1:Name};"),
            ("enum", "enum ${1:Name} {\n\t$0\n};"),
            (
                "guard",
                "#ifndef ${1:NAME_H}\n#define ${1:NAME_H}\n\n$0\n\n#endif",
            ),
        ],
    ),
    ("C++", STOPS_C_LIKE),
    (
        "C++",
        &[
            ("main", "int main() {\n\t$0\n\treturn 0;\n}"),
            ("include", "#include <${1:iostream}>"),
            ("class", "class ${1:Name} {\npublic:\n\t$0\n};"),
            ("struct", "struct ${1:Name} {\n\t$0\n};"),
            (
                "forrange",
                "for (const auto& ${1:item} : ${2:items}) {\n\t$0\n}",
            ),
            ("cout", "std::cout << ${1:value} << std::endl;"),
            ("function", "${1:void} ${2:name}($3) {\n\t$0\n}"),
            ("method", "${1:void} ${2:Class}::${3:name}($4) {\n\t$0\n}"),
            ("namespace", "namespace ${1:name} {\n\n$0\n\n}"),
            ("lambda", "auto ${1:name} = [${2:&}]($3) {\n\t$0\n};"),
            (
                "try",
                "try {\n\t$1\n} catch (const std::exception& ${2:error}) {\n\t$0\n}",
            ),
            ("template", "template <typename ${1:T}>"),
            (
                "unique",
                "auto ${1:name} = std::make_unique<${2:Type}>($3);",
            ),
        ],
    ),
    ("Java", STOPS_C_LIKE),
    (
        "Java",
        &[
            ("main", "public static void main(String[] args) {\n\t$0\n}"),
            ("class", "public class ${1:Name} {\n\t$0\n}"),
            ("foreach", "for (var ${1:item} : ${2:items}) {\n\t$0\n}"),
            ("sout", "System.out.println($0);"),
            (
                "try",
                "try {\n\t$1\n} catch (${2:Exception} ${3:e}) {\n\t$0\n}",
            ),
            ("method", "public ${1:void} ${2:name}($3) {\n\t$0\n}"),
            ("constructor", "public ${1:Name}($2) {\n\t$0\n}"),
            ("interface", "public interface ${1:Name} {\n\t$0\n}"),
            ("enum", "public enum ${1:Name} {\n\t$0\n}"),
            ("record", "public record ${1:Name}($2) {$0}"),
            (
                "tryfinally",
                "try {\n\t$1\n} catch (${2:Exception} ${3:e}) {\n\t$4\n} finally {\n\t$0\n}",
            ),
            ("test", "@Test\nvoid ${1:name}() {\n\t$0\n}"),
            (
                "constant",
                "private static final ${1:String} ${2:NAME} = ${3:null};",
            ),
        ],
    ),
    ("C#", STOPS_C_LIKE),
    (
        "C#",
        &[
            ("main", "static void Main(string[] args)\n{\n\t$0\n}"),
            ("class", "public class ${1:Name}\n{\n\t$0\n}"),
            (
                "foreach",
                "foreach (var ${1:item} in ${2:items})\n{\n\t$0\n}",
            ),
            ("prop", "public ${1:string} ${2:Name} { get; set; }"),
            ("cw", "Console.WriteLine($0);"),
            (
                "try",
                "try\n{\n\t$1\n}\ncatch (${2:Exception} ${3:e})\n{\n\t$0\n}",
            ),
            ("method", "public ${1:void} ${2:Name}($3)\n{\n\t$0\n}"),
            ("constructor", "public ${1:Name}($2)\n{\n\t$0\n}"),
            ("interface", "public interface ${1:IName}\n{\n\t$0\n}"),
            ("enum", "public enum ${1:Name}\n{\n\t$0\n}"),
            ("record", "public record ${1:Name}($2);"),
            ("async", "public async Task ${1:NameAsync}($2)\n{\n\t$0\n}"),
            (
                "tryfinally",
                "try\n{\n\t$1\n}\ncatch (${2:Exception} ${3:e})\n{\n\t$4\n}\nfinally\n{\n\t$0\n}",
            ),
            ("namespace", "namespace ${1:Name}\n{\n\t$0\n}"),
            (
                "using",
                "using (var ${1:resource} = ${2:value})\n{\n\t$0\n}",
            ),
        ],
    ),
    (
        "Kotlin",
        &[
            ("main", "fun main() {\n\t$0\n}"),
            ("fun", "fun ${1:name}($2) {\n\t$0\n}"),
            ("class", "class ${1:Name} {\n\t$0\n}"),
            ("for", "for (${1:item} in ${2:items}) {\n\t$0\n}"),
            ("if", "if (${1:condition}) {\n\t$0\n}"),
            ("when", "when (${1:value}) {\n\t${2:else} -> {$0}\n}"),
            ("println", "println($0)"),
            ("ifelse", "if (${1:condition}) {\n\t$2\n} else {\n\t$0\n}"),
            (
                "dataclass",
                "data class ${1:Name}(val ${2:name}: ${3:String})",
            ),
            ("interface", "interface ${1:Name} {\n\t$0\n}"),
            (
                "try",
                "try {\n\t$1\n} catch (${2:e}: ${3:Exception}) {\n\t$0\n}",
            ),
        ],
    ),
    (
        "Swift",
        &[
            ("func", "func ${1:name}($2) {\n\t$0\n}"),
            ("struct", "struct ${1:Name} {\n\t$0\n}"),
            ("class", "class ${1:Name} {\n\t$0\n}"),
            ("for", "for ${1:item} in ${2:items} {\n\t$0\n}"),
            ("if", "if ${1:condition} {\n\t$0\n}"),
            ("guard", "guard ${1:condition} else {\n\t${0:return}\n}"),
            ("print", "print($0)"),
            ("iflet", "if let ${1:value} = ${2:optional} {\n\t$0\n}"),
            (
                "guardlet",
                "guard let ${1:value} = ${2:optional} else {\n\t${0:return}\n}",
            ),
            ("enum", "enum ${1:Name} {\n\t$0\n}"),
            ("protocol", "protocol ${1:Name} {\n\t$0\n}"),
            ("extension", "extension ${1:Type} {\n\t$0\n}"),
            (
                "switch",
                "switch ${1:value} {\ncase ${2:pattern}:\n\t$3\ndefault:\n\t${0:break}\n}",
            ),
            ("docatch", "do {\n\t$1\n} catch {\n\t$0\n}"),
        ],
    ),
    (
        "PHP",
        &[
            ("function", "function ${1:name}($2) {\n\t$0\n}"),
            ("class", "class ${1:Name} {\n\t$0\n}"),
            (
                "foreach",
                "foreach (\\$${1:items} as \\$${2:item}) {\n\t$0\n}",
            ),
            ("if", "if (${1:condition}) {\n\t$0\n}"),
            ("echo", "echo $0;"),
            ("ifelse", "if (${1:condition}) {\n\t$2\n} else {\n\t$0\n}"),
            (
                "for",
                "for (\\$${1:i} = 0; \\$${1:i} < ${2:10}; \\$${1:i}++) {\n\t$0\n}",
            ),
            ("method", "public function ${1:name}($2) {\n\t$0\n}"),
            ("constructor", "public function __construct($1) {\n\t$0\n}"),
            (
                "try",
                "try {\n\t$1\n} catch (${2:Exception} \\$${3:error}) {\n\t$0\n}",
            ),
        ],
    ),
    (
        "Ruby",
        &[
            ("def", "def ${1:name}\n\t$0\nend"),
            ("class", "class ${1:Name}\n\t$0\nend"),
            ("each", "${1:items}.each do |${2:item}|\n\t$0\nend"),
            ("if", "if ${1:condition}\n\t$0\nend"),
            ("puts", "puts $0"),
            ("ifelse", "if ${1:condition}\n\t$2\nelse\n\t$0\nend"),
            ("module", "module ${1:Name}\n\t$0\nend"),
            ("initialize", "def initialize($1)\n\t$0\nend"),
            ("map", "${1:items}.map { |${2:item}| ${3:item} }"),
            (
                "begin",
                "begin\n\t$1\nrescue ${2:StandardError} => ${3:error}\n\t$0\nend",
            ),
        ],
    ),
    (
        "Shell",
        &[
            ("if", "if [ ${1:condition} ]; then\n\t$0\nfi"),
            ("for", "for ${1:item} in ${2:items}; do\n\t$0\ndone"),
            ("while", "while ${1:condition}; do\n\t$0\ndone"),
            ("function", "${1:name}() {\n\t$0\n}"),
            (
                "case",
                "case ${1:value} in\n\t${2:*})\n\t\t$0\n\t\t;;\nesac",
            ),
            (
                "ifelse",
                "if [ ${1:condition} ]; then\n\t$2\nelse\n\t$0\nfi",
            ),
            ("bash", "#!/usr/bin/env bash\nset -euo pipefail\n\n$0"),
            (
                "readlines",
                "while IFS= read -r ${1:line}; do\n\t$0\ndone < ${2:file}",
            ),
        ],
    ),
    (
        "SQL",
        &[
            ("select", "SELECT ${1:*} FROM ${2:table};"),
            (
                "insert",
                "INSERT INTO ${1:table} (${2:columns}) VALUES (${3:values});",
            ),
            (
                "update",
                "UPDATE ${1:table} SET ${2:column} = ${3:value} WHERE ${4:condition};",
            ),
            (
                "create",
                "CREATE TABLE ${1:name} (\n\tid INTEGER PRIMARY KEY$0\n);",
            ),
            (
                "selectwhere",
                "SELECT ${1:*} FROM ${2:table} WHERE ${3:condition};",
            ),
            ("delete", "DELETE FROM ${1:table} WHERE ${2:condition};"),
            ("join", "JOIN ${1:table} ON ${2:condition}"),
            (
                "index",
                "CREATE INDEX ${1:name} ON ${2:table} (${3:column});",
            ),
            (
                "alter",
                "ALTER TABLE ${1:table} ADD COLUMN ${2:name} ${3:TEXT};",
            ),
        ],
    ),
];

/// The snippets with tab stops that come with `language`.
fn stops_snippets_of(language: &str) -> impl Iterator<Item = (&'static str, &'static str)> {
    let own = STOPS_SNIPPETS.iter();
    let own = own.filter(move |(name, _)| *name == language);
    own.flat_map(|(_, snippets)| snippets.iter().copied())
}

/// What JavaScript comes with, in a browser or in Node.
const SCRIPT_BUILTINS: &[&str] = &[
    "console",
    "document",
    "window",
    "globalThis",
    "Promise",
    "JSON",
    "Math",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Symbol",
    "BigInt",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Date",
    "RegExp",
    "Error",
    "TypeError",
    "RangeError",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "setTimeout",
    "clearTimeout",
    "setInterval",
    "clearInterval",
    "requestAnimationFrame",
    "queueMicrotask",
    "structuredClone",
    "fetch",
    "URL",
    "URLSearchParams",
    "FormData",
    "Headers",
    "Request",
    "Response",
    "AbortController",
    "localStorage",
    "sessionStorage",
    "navigator",
    "location",
    "history",
    "alert",
    "process",
    "require",
    "module",
    "exports",
    "encodeURIComponent",
    "decodeURIComponent",
    "Intl",
    "Uint8Array",
    "ArrayBuffer",
    "TextEncoder",
    "TextDecoder",
    "undefined",
    "NaN",
    "Infinity",
];

/// The names a language comes with that are not keywords, by language name: the types,
/// functions and macros most often written. A macro of Rust ends with its `!`.
const BUILTINS: &[(&str, &[&str])] = &[
    (
        "Rust",
        &[
            "String",
            "Vec",
            "Option",
            "Result",
            "Some",
            "None",
            "Ok",
            "Err",
            "Box",
            "Rc",
            "Arc",
            "Cell",
            "RefCell",
            "Mutex",
            "RwLock",
            "HashMap",
            "HashSet",
            "BTreeMap",
            "BTreeSet",
            "VecDeque",
            "Cow",
            "Path",
            "PathBuf",
            "Duration",
            "Instant",
            "Iterator",
            "IntoIterator",
            "Default",
            "Clone",
            "Copy",
            "Debug",
            "Display",
            "PartialEq",
            "Eq",
            "PartialOrd",
            "Ord",
            "Hash",
            "From",
            "Into",
            "TryFrom",
            "TryInto",
            "AsRef",
            "ToString",
            "ToOwned",
            "Send",
            "Sync",
            "Sized",
            "Drop",
            "Fn",
            "FnMut",
            "FnOnce",
            "usize",
            "isize",
            "u8",
            "u16",
            "u32",
            "u64",
            "u128",
            "i8",
            "i16",
            "i32",
            "i64",
            "i128",
            "f32",
            "f64",
            "bool",
            "char",
            "str",
            "println!",
            "eprintln!",
            "print!",
            "format!",
            "vec!",
            "assert!",
            "assert_eq!",
            "assert_ne!",
            "debug_assert!",
            "panic!",
            "todo!",
            "unimplemented!",
            "unreachable!",
            "matches!",
            "write!",
            "writeln!",
            "dbg!",
            "include_str!",
            "concat!",
            "env!",
            "cfg!",
        ],
    ),
    (
        "Python",
        &[
            "abs",
            "all",
            "any",
            "bool",
            "bytes",
            "callable",
            "chr",
            "classmethod",
            "dict",
            "dir",
            "enumerate",
            "filter",
            "float",
            "format",
            "frozenset",
            "getattr",
            "hasattr",
            "hash",
            "id",
            "input",
            "int",
            "isinstance",
            "issubclass",
            "iter",
            "len",
            "list",
            "map",
            "max",
            "min",
            "next",
            "object",
            "open",
            "ord",
            "pow",
            "print",
            "property",
            "range",
            "repr",
            "reversed",
            "round",
            "set",
            "setattr",
            "slice",
            "sorted",
            "staticmethod",
            "str",
            "sum",
            "super",
            "tuple",
            "type",
            "zip",
            "Exception",
            "ValueError",
            "TypeError",
            "KeyError",
            "IndexError",
            "AttributeError",
            "RuntimeError",
            "StopIteration",
            "NotImplementedError",
            "FileNotFoundError",
            "OSError",
            "ImportError",
            "__init__",
            "__name__",
            "__main__",
            "__str__",
            "__repr__",
            "__len__",
            "__eq__",
        ],
    ),
    ("JavaScript", SCRIPT_BUILTINS),
    ("TypeScript", SCRIPT_BUILTINS),
    (
        "TypeScript",
        &[
            "Partial",
            "Required",
            "Readonly",
            "Record",
            "Pick",
            "Omit",
            "Exclude",
            "Extract",
            "NonNullable",
            "ReturnType",
            "Parameters",
            "Awaited",
            "string",
            "number",
            "boolean",
            "unknown",
            "never",
            "any",
            "object",
            "bigint",
        ],
    ),
    (
        "Go",
        &[
            "append", "cap", "clear", "close", "copy", "delete", "len", "make", "max", "min",
            "new", "panic", "print", "println", "recover", "string", "int", "int8", "int16",
            "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "byte", "rune",
            "float32", "float64", "bool", "error", "any", "nil", "true", "false", "iota", "fmt",
            "errors", "strings", "strconv", "context", "time",
        ],
    ),
    (
        "Java",
        &[
            "String",
            "Integer",
            "Long",
            "Double",
            "Float",
            "Boolean",
            "Character",
            "Object",
            "Math",
            "System",
            "StringBuilder",
            "List",
            "ArrayList",
            "LinkedList",
            "Map",
            "HashMap",
            "TreeMap",
            "Set",
            "HashSet",
            "Optional",
            "Arrays",
            "Collections",
            "Objects",
            "Stream",
            "Collectors",
            "Iterator",
            "Iterable",
            "Comparable",
            "Comparator",
            "Runnable",
            "Thread",
            "Exception",
            "RuntimeException",
            "IllegalArgumentException",
            "IllegalStateException",
            "NullPointerException",
            "IOException",
            "Override",
            "Deprecated",
            "FunctionalInterface",
            "LocalDate",
            "LocalDateTime",
            "Duration",
            "Instant",
            "Files",
            "Scanner",
        ],
    ),
    (
        "C",
        &[
            "printf",
            "fprintf",
            "sprintf",
            "snprintf",
            "scanf",
            "sscanf",
            "puts",
            "putchar",
            "getchar",
            "fgets",
            "fputs",
            "fopen",
            "fclose",
            "fread",
            "fwrite",
            "malloc",
            "calloc",
            "realloc",
            "free",
            "memcpy",
            "memmove",
            "memset",
            "memcmp",
            "strlen",
            "strcpy",
            "strncpy",
            "strcat",
            "strcmp",
            "strncmp",
            "strchr",
            "strstr",
            "atoi",
            "atof",
            "strtol",
            "exit",
            "abort",
            "assert",
            "qsort",
            "size_t",
            "ssize_t",
            "uint8_t",
            "uint16_t",
            "uint32_t",
            "uint64_t",
            "int8_t",
            "int16_t",
            "int32_t",
            "int64_t",
            "bool",
            "NULL",
            "EOF",
            "FILE",
            "stdin",
            "stdout",
            "stderr",
            "EXIT_SUCCESS",
            "EXIT_FAILURE",
        ],
    ),
    (
        "C++",
        &[
            "std",
            "string",
            "string_view",
            "vector",
            "map",
            "unordered_map",
            "set",
            "unordered_set",
            "array",
            "span",
            "pair",
            "tuple",
            "optional",
            "variant",
            "unique_ptr",
            "shared_ptr",
            "make_unique",
            "make_shared",
            "move",
            "forward",
            "cout",
            "cerr",
            "cin",
            "endl",
            "begin",
            "end",
            "sort",
            "find",
            "swap",
            "min",
            "max",
            "function",
            "thread",
            "mutex",
            "lock_guard",
            "to_string",
            "exception",
            "runtime_error",
            "size_t",
            "uint8_t",
            "uint32_t",
            "uint64_t",
            "int32_t",
            "int64_t",
            "printf",
        ],
    ),
    (
        "C#",
        &[
            "Console",
            "String",
            "Math",
            "Convert",
            "DateTime",
            "TimeSpan",
            "Guid",
            "List",
            "Dictionary",
            "HashSet",
            "Queue",
            "Stack",
            "IEnumerable",
            "IList",
            "IDictionary",
            "IDisposable",
            "Task",
            "ValueTask",
            "CancellationToken",
            "Action",
            "Func",
            "Exception",
            "ArgumentException",
            "ArgumentNullException",
            "InvalidOperationException",
            "NotImplementedException",
            "StringBuilder",
            "File",
            "Path",
            "Directory",
            "Enumerable",
            "Nullable",
            "Tuple",
            "Environment",
            "Object",
            "Array",
            "Encoding",
            "Regex",
            "Stopwatch",
        ],
    ),
    (
        "PHP",
        &[
            "array_map",
            "array_filter",
            "array_merge",
            "array_keys",
            "array_values",
            "array_key_exists",
            "in_array",
            "count",
            "isset",
            "empty",
            "strlen",
            "str_replace",
            "str_contains",
            "substr",
            "strpos",
            "explode",
            "implode",
            "trim",
            "sprintf",
            "printf",
            "var_dump",
            "print_r",
            "json_encode",
            "json_decode",
            "is_array",
            "is_string",
            "is_null",
            "is_numeric",
            "intval",
            "strval",
            "file_get_contents",
            "file_put_contents",
            "date",
            "time",
            "preg_match",
            "preg_replace",
            "header",
            "Exception",
            "DateTime",
            "PDO",
        ],
    ),
    (
        "Ruby",
        &[
            "puts",
            "print",
            "require",
            "require_relative",
            "attr_accessor",
            "attr_reader",
            "attr_writer",
            "raise",
            "lambda",
            "proc",
            "loop",
            "format",
            "gets",
            "initialize",
            "Array",
            "Hash",
            "String",
            "Integer",
            "Float",
            "Symbol",
            "Struct",
            "Time",
            "File",
            "Dir",
            "Kernel",
            "Comparable",
            "Enumerable",
            "StandardError",
            "ArgumentError",
            "RuntimeError",
            "NotImplementedError",
        ],
    ),
    (
        "Kotlin",
        &[
            "println",
            "print",
            "listOf",
            "mutableListOf",
            "mapOf",
            "mutableMapOf",
            "setOf",
            "mutableSetOf",
            "arrayOf",
            "emptyList",
            "emptyMap",
            "lazy",
            "require",
            "check",
            "error",
            "TODO",
            "repeat",
            "run",
            "let",
            "apply",
            "also",
            "with",
            "String",
            "Int",
            "Long",
            "Double",
            "Float",
            "Boolean",
            "Char",
            "Unit",
            "Any",
            "Nothing",
            "List",
            "MutableList",
            "Map",
            "MutableMap",
            "Set",
            "Pair",
            "Triple",
            "Array",
            "Sequence",
            "Exception",
            "IllegalArgumentException",
            "IllegalStateException",
        ],
    ),
    (
        "Swift",
        &[
            "print",
            "debugPrint",
            "fatalError",
            "precondition",
            "assert",
            "min",
            "max",
            "abs",
            "zip",
            "stride",
            "String",
            "Int",
            "Double",
            "Float",
            "Bool",
            "Character",
            "Array",
            "Dictionary",
            "Set",
            "Optional",
            "Result",
            "Error",
            "Void",
            "Any",
            "AnyObject",
            "Data",
            "Date",
            "URL",
            "UUID",
            "Codable",
            "Equatable",
            "Hashable",
            "Comparable",
            "Identifiable",
            "CustomStringConvertible",
            "DispatchQueue",
            "Task",
        ],
    ),
    (
        "SQL",
        &[
            "COUNT",
            "SUM",
            "AVG",
            "MIN",
            "MAX",
            "COALESCE",
            "NULLIF",
            "CAST",
            "CONCAT",
            "LENGTH",
            "LOWER",
            "UPPER",
            "TRIM",
            "SUBSTRING",
            "REPLACE",
            "ROUND",
            "ABS",
            "NOW",
            "CURRENT_DATE",
            "CURRENT_TIMESTAMP",
            "ROW_NUMBER",
            "RANK",
            "INTEGER",
            "BIGINT",
            "TEXT",
            "VARCHAR",
            "BOOLEAN",
            "DATE",
            "TIMESTAMP",
            "NUMERIC",
            "DECIMAL",
            "REAL",
            "SERIAL",
            "UUID",
            "JSONB",
        ],
    ),
    (
        "Shell",
        &[
            "echo", "printf", "read", "cd", "pwd", "export", "source", "exit", "test", "set",
            "unset", "shift", "local", "readonly", "trap", "eval", "exec", "grep", "sed", "awk",
            "cat", "ls", "cp", "mv", "rm", "mkdir", "rmdir", "touch", "chmod", "chown", "find",
            "xargs", "sort", "uniq", "head", "tail", "wc", "cut", "tr", "curl", "wget", "tar",
            "basename", "dirname", "sleep", "kill", "date", "true", "false",
        ],
    ),
];

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Char index where the word ending at `caret` starts.
pub fn word_start(text: &str, caret: usize) -> usize {
    let before = &text[..byte_index(text, caret)];
    caret
        - before
            .chars()
            .rev()
            .take_while(|ch| is_word_char(*ch))
            .count()
}

/// What the word ending at `caret` could become, best first. Empty when no word is there.
pub fn complete(text: &str, caret: usize, language: &Language) -> Vec<Completion> {
    let end = byte_index(text, caret);
    let prefix = &text[byte_index(text, word_start(text, caret))..end];
    if prefix.is_empty() || prefix.starts_with(|ch: char| ch.is_ascii_digit()) {
        return Vec::new();
    }
    let wanted = prefix.to_lowercase();
    let fits = |label: &str| label.to_lowercase().starts_with(&wanted);
    let found = placed(text, end, prefix, language);
    if !found.is_empty() {
        return found;
    }

    let mut found: Vec<Completion> = snippets_of(language.name)
        .filter(|(label, _)| fits(label))
        .map(|(label, body)| completion(label, body, Kind::Snippet))
        .collect();
    // The one named by what is typed comes first: Tab then writes it.
    found.sort_by_key(|snippet| !snippet.label.eq_ignore_ascii_case(prefix));

    // A word typed in full has nothing left to complete.
    let mut seen: HashSet<&str> = HashSet::from([prefix]);
    for keyword in language.keywords() {
        if fits(keyword) && seen.insert(keyword) {
            found.push(completion(keyword, keyword, Kind::Keyword));
        }
    }
    let mut words: Vec<&str> = text
        .split(|ch: char| !is_word_char(ch))
        .filter(|word| word.chars().count() >= MIN_WORD_CHARS && fits(word))
        .filter(|word| seen.insert(word))
        .collect();
    words.sort_by_key(|word| (word.len(), *word));
    found.extend(
        words
            .into_iter()
            .map(|word| completion(word, word, Kind::Word)),
    );
    found.truncate(MAX_COMPLETIONS);
    found
}

fn completion(label: &str, body: &str, kind: Kind) -> Completion {
    Completion {
        label: label.to_owned(),
        body: body.to_owned(),
        kind,
    }
}

/// What markup and style sheets offer for the word `prefix` that ends at the byte `end`:
/// attributes in a tag, values in an attribute, properties and values in a style sheet.
/// Empty anywhere else.
fn placed(text: &str, end: usize, prefix: &str, language: &Language) -> Vec<Completion> {
    // The name being written with its hyphens: of `font-s`, the word is `s` alone.
    let before = &text[..end];
    let named = before
        .chars()
        .rev()
        .take_while(|ch| is_word_char(*ch) || *ch == '-');
    let full = &before[end - named.map(char::len_utf8).sum::<usize>()..];
    let lead = full.len() - prefix.len();
    // Those of `names` the name being written starts, each written from the word on.
    let offered = |names: &mut dyn Iterator<Item = &'static str>, body: &dyn Fn(&str) -> String| {
        let wanted = full.to_lowercase();
        let fitting = names.filter(|name| name.len() > lead && name.starts_with(&wanted));
        let made = fitting.map(|name| completion(name, &body(name)[lead..], Kind::Keyword));
        made.take(MAX_COMPLETIONS).collect::<Vec<_>>()
    };
    let around = &before[..end - full.len()];
    let place = match language.is_markup() {
        true => place_in_markup(around),
        false if language.name == "CSS" => Some(Place::Style(around, false)),
        false => None,
    };
    match place {
        // In a tag: the attributes of its element, then those of any element.
        Some(Place::Attribute(element)) => {
            let own = ATTRIBUTES.iter().filter(|(of, _)| *of == element);
            let known = own.chain(&ATTRIBUTES[..1]);
            let mut names = known.flat_map(|(_, names)| names.iter().copied());
            // One that is there or not takes no value.
            offered(&mut names, &|name| match SWITCHES.contains(&name) {
                true => name.to_owned(),
                false => format!("{name}=\"$0\""),
            })
        }
        Some(Place::Value(attribute)) => {
            let own = ATTRIBUTE_VALUES.iter().filter(|(of, _)| *of == attribute);
            let mut values = own.flat_map(|(_, values)| values.iter().copied());
            offered(&mut values, &str::to_owned)
        }
        // In a style sheet: a property where a declaration starts, its values after it.
        Some(Place::Style(sheet, inline)) => match declaration(sheet, inline) {
            Some(None) => {
                let mut names = CSS_PROPERTIES.iter().copied();
                offered(&mut names, &|name| format!("{name}: $0;"))
            }
            Some(Some(property)) => {
                let own = CSS_VALUES.iter().filter(|(of, _)| *of == property);
                let known = own.chain(&CSS_VALUES[..1]);
                let mut values = known.flat_map(|(_, values)| values.iter().copied());
                offered(&mut values, &str::to_owned)
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// The attributes of the elements, by element; the first are those of all of them.
const ATTRIBUTES: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "class",
            "id",
            "style",
            "title",
            "hidden",
            "lang",
            "tabindex",
            "role",
            "aria-label",
            "data-",
        ],
    ),
    ("a", &["href", "target", "rel", "download"]),
    ("img", &["src", "alt", "width", "height", "loading"]),
    (
        "input",
        &[
            "type",
            "name",
            "value",
            "placeholder",
            "required",
            "disabled",
            "checked",
            "min",
            "max",
        ],
    ),
    ("form", &["action", "method"]),
    ("button", &["type", "disabled"]),
    ("link", &["rel", "href"]),
    ("script", &["src", "type", "defer", "async"]),
    ("meta", &["name", "content", "charset"]),
    ("label", &["for"]),
    ("select", &["name", "required", "disabled"]),
    (
        "textarea",
        &["name", "rows", "cols", "placeholder", "required"],
    ),
    ("option", &["value", "selected"]),
    ("td", &["colspan", "rowspan"]),
    ("th", &["colspan", "rowspan", "scope"]),
    ("iframe", &["src", "width", "height"]),
    (
        "video",
        &["src", "controls", "autoplay", "loop", "muted", "poster"],
    ),
    ("audio", &["src", "controls", "autoplay", "loop", "muted"]),
    ("source", &["src", "type"]),
];

/// The attributes that are there or not, and take no value.
const SWITCHES: [&str; 12] = [
    "hidden", "required", "disabled", "checked", "selected", "defer", "async", "controls",
    "autoplay", "loop", "muted", "download",
];

/// The properties of a style sheet most often written.
const CSS_PROPERTIES: [&str; 62] = [
    "align-items",
    "align-self",
    "background",
    "background-color",
    "background-image",
    "border",
    "border-bottom",
    "border-color",
    "border-left",
    "border-radius",
    "border-right",
    "border-top",
    "bottom",
    "box-shadow",
    "box-sizing",
    "color",
    "cursor",
    "display",
    "flex",
    "flex-direction",
    "flex-wrap",
    "float",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "gap",
    "grid-template-columns",
    "grid-template-rows",
    "height",
    "justify-content",
    "left",
    "letter-spacing",
    "line-height",
    "list-style",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "opacity",
    "outline",
    "overflow",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "position",
    "right",
    "text-align",
    "text-decoration",
    "text-transform",
    "top",
    "transform",
    "transition",
    "width",
    "z-index",
];

/// The words a property takes as values; the first are those any of them takes.
const CSS_VALUES: &[(&str, &[&str])] = &[
    ("", &["inherit", "initial", "unset", "auto", "none"]),
    (
        "display",
        &[
            "block",
            "inline",
            "inline-block",
            "flex",
            "grid",
            "inline-flex",
            "contents",
        ],
    ),
    (
        "position",
        &["static", "relative", "absolute", "fixed", "sticky"],
    ),
    (
        "flex-direction",
        &["row", "column", "row-reverse", "column-reverse"],
    ),
    ("flex-wrap", &["wrap", "nowrap"]),
    (
        "justify-content",
        &[
            "center",
            "flex-start",
            "flex-end",
            "space-between",
            "space-around",
            "space-evenly",
        ],
    ),
    (
        "align-items",
        &["center", "flex-start", "flex-end", "stretch", "baseline"],
    ),
    (
        "align-self",
        &["center", "flex-start", "flex-end", "stretch", "baseline"],
    ),
    ("text-align", &["left", "right", "center", "justify"]),
    (
        "text-decoration",
        &["underline", "line-through", "overline"],
    ),
    ("text-transform", &["uppercase", "lowercase", "capitalize"]),
    ("font-weight", &["bold", "normal", "lighter", "bolder"]),
    ("font-style", &["italic", "normal", "oblique"]),
    ("overflow", &["hidden", "visible", "scroll"]),
    (
        "cursor",
        &["pointer", "default", "text", "move", "not-allowed", "grab"],
    ),
    ("box-sizing", &["border-box", "content-box"]),
    ("float", &["left", "right"]),
    ("list-style", &["disc", "circle", "square", "decimal"]),
];

/// The values some attributes take.
const ATTRIBUTE_VALUES: &[(&str, &[&str])] = &[
    (
        "type",
        &[
            "text", "password", "email", "number", "checkbox", "radio", "submit", "button",
            "reset", "file", "date", "color", "range", "hidden", "search", "tel", "url",
        ],
    ),
    ("target", &["_blank", "_self", "_parent", "_top"]),
    (
        "rel",
        &["stylesheet", "icon", "noopener", "noreferrer", "nofollow"],
    ),
    ("method", &["get", "post"]),
    ("loading", &["lazy", "eager"]),
    ("charset", &["utf-8"]),
    ("scope", &["col", "row"]),
];

/// Where a name is being written in markup.
enum Place<'a> {
    /// In the tag of this element, where its attributes go.
    Attribute(&'a str),
    /// Between the quotes of the value of this attribute.
    Value(&'a str),
    /// In a style sheet, of which this is the part before the name: the content of a
    /// `style` element, or of a `style` attribute (`true`), which is one block.
    Style(&'a str, bool),
}

/// Where the name that follows `before` is being written, when that says what it can be.
fn place_in_markup(before: &str) -> Option<Place<'_>> {
    let tag = before.rfind('<').map(|at| &before[at + 1..]);
    if let Some(tag) = tag.filter(|tag| !tag.contains('>')) {
        if tag.starts_with(['/', '!', '?']) {
            return None;
        }
        // Its name is written once something follows it.
        let (name, rest) = tag.split_once(char::is_whitespace)?;
        if name.is_empty() || !name.chars().all(in_tag_name) {
            return None;
        }
        if rest.matches('"').count() % 2 == 0 {
            return Some(Place::Attribute(name));
        }
        let quote = rest.rfind('"')?;
        let attribute = rest[..quote].strip_suffix('=')?;
        let attribute = attribute.rsplit(char::is_whitespace).next()?;
        return Some(match attribute {
            "style" => Place::Style(&rest[quote + 1..], true),
            attribute => Place::Value(attribute),
        });
    }
    Some(Place::Style(style_sheet(before)?, false))
}

/// The style sheet `before`, a page up to some place, ends in: what its `style` element
/// holds so far. `None` outside such an element.
pub fn style_sheet(before: &str) -> Option<&str> {
    let sheet = &before[before.rfind("<style")?..];
    if sheet.contains("</style") {
        return None;
    }
    Some(&sheet[sheet.find('>')? + 1..])
}

/// What is being written at the end of `sheet`, a style sheet up to a name: a property
/// (`Some(None)`), or a value of the property named; `None` outside a block, where
/// selectors are. `inline`: the sheet is one block, as in a `style` attribute.
fn declaration(sheet: &str, inline: bool) -> Option<Option<&str>> {
    let in_block = inline || sheet.matches('{').count() > sheet.matches('}').count();
    if !in_block {
        return None;
    }
    let started = sheet.rsplit(['{', ';', '\n']).next().unwrap_or("").trim();
    if started.is_empty() {
        return Some(None);
    }
    let (property, _) = started.split_once(':')?;
    Some(Some(property.trim()))
}

/// A tag of a text: where its name is, in bytes, whether it closes an element, and
/// whether it holds nothing and so goes with no other.
struct Tag {
    name: Range<usize>,
    closes: bool,
    alone: bool,
    /// It is written to its end: one still being typed goes with no other yet.
    whole: bool,
}

fn tags_of(text: &str) -> Vec<Tag> {
    let found = text.match_indices('<').filter_map(|(at, _)| {
        let rest = &text[at + 1..];
        let closes = rest.starts_with('/');
        let start = at + 1 + usize::from(closes);
        let length: usize = text[start..]
            .chars()
            .take_while(|ch| in_tag_name(*ch))
            .map(char::len_utf8)
            .sum();
        let name = &text[start..start + length];
        // `a < b` is no tag; `</>` is one whose name was taken out.
        let after = text[start + length..].chars().next();
        let tag = after.is_some_and(|ch| ch == '>' || ch == '/' || ch.is_whitespace());
        if !tag || (name.is_empty() && !closes && after != Some('>')) {
            return None;
        }
        let end = rest
            .find(['<', '>'])
            .filter(|end| rest[*end..].starts_with('>'));
        let alone = end.is_some_and(|end| rest[..end].ends_with('/'))
            || crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str());
        Some(Tag {
            name: start..start + length,
            closes,
            alone: alone && !closes,
            whole: end.is_some(),
        })
    });
    found.collect()
}

/// Whether one name is the other with one char more.
fn one_char_apart(one: &str, other: &str) -> bool {
    let (long, short) = match one.len() > other.len() {
        true => (one, other),
        false => (other, one),
    };
    let same = long.chars().zip(short.chars()).take_while(|(a, b)| a == b);
    let prefix: usize = same.map(|(ch, _)| ch.len_utf8()).sum();
    let skipped = long[prefix..].chars().next().map_or(0, char::len_utf8);
    long.len() != short.len() && long[prefix + skipped..] == short[prefix..]
}

/// After the name of a tag was edited at the byte `caret`: where the name of the tag that
/// goes with it is, and the name to write there for the two to go together again. `None`
/// when the caret is in no name, or the tag that would go with it bore another name.
pub fn twin_tag(text: &str, caret: usize) -> Option<(Range<usize>, String)> {
    let all = tags_of(text);
    let at = all
        .iter()
        .position(|tag| tag.name.start <= caret && caret <= tag.name.end)?;
    let edited = &all[at];
    if edited.alone || !edited.whole {
        return None;
    }
    // The tag that goes with it: the first one, the way it looks, that pairs with none
    // of those between.
    let mut depth = 0_usize;
    let pairs = |tag: &&Tag| {
        if tag.alone || !tag.whole {
            return false;
        }
        if tag.closes == edited.closes {
            depth += 1;
            return false;
        }
        let found = depth == 0;
        depth = depth.saturating_sub(1);
        found
    };
    let twin = match edited.closes {
        false => all[at + 1..].iter().find(pairs),
        true => all[..at].iter().rev().find(pairs),
    }?;
    let (name, other) = (&text[edited.name.clone()], &text[twin.name.clone()]);
    one_char_apart(name, other).then(|| (twin.name.clone(), name.to_owned()))
}

/// What a tag may be named with.
fn in_tag_name(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '-' | ':' | '.')
}

/// The tags named `name` in `text`, in order: `true` for one that opens an element, `false`
/// for one that closes it. A tag that closes itself (`<a />`) is neither.
fn tags_named<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = bool> + 'a {
    text.match_indices('<').filter_map(move |(at, _)| {
        let rest = &text[at + 1..];
        let (closes, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let found = rest.get(..name.len())?;
        let longer = rest[name.len()..].chars().next().is_some_and(in_tag_name);
        if !found.eq_ignore_ascii_case(name) || longer {
            return None;
        }
        let empty = rest.find('>').is_some_and(|end| rest[..end].ends_with('/'));
        (closes || !empty).then_some(!closes)
    })
}

/// The closing tag to write after a `>` typed between `before` and `after`, when that `>`
/// ends an opening tag that wants one: not when a closing tag after it is already waiting
/// for it.
pub fn closing_tag(before: &str, after: &str) -> Option<String> {
    let opened = before.rfind('<')?;
    let tag = &before[opened + 1..];
    if tag.contains('>') || tag.ends_with('/') {
        return None;
    }
    let name: String = tag.chars().take_while(|ch| in_tag_name(*ch)).collect();
    let void = crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str());
    if name.is_empty() || void {
        return None;
    }
    // Elements of that name still open before this one: each has a closing tag coming.
    let open = tags_named(&before[..opened], &name).fold(0_usize, |open, opens| match opens {
        true => open + 1,
        false => open.saturating_sub(1),
    });
    // Closing tags after it that close nothing opened there: one more of them than of
    // those elements, and it is the one this tag was missing.
    let (mut depth, mut spare) = (0_usize, 0_usize);
    for opens in tags_named(after, &name) {
        match (opens, depth) {
            (true, _) => depth += 1,
            (false, 0) => spare += 1,
            (false, _) => depth -= 1,
        }
    }
    (spare <= open).then(|| format!("</{name}>"))
}

/// The element still open at the end of `before`: its name, and the byte its opening tag
/// starts at.
pub fn unclosed_tag(before: &str) -> Option<(String, usize)> {
    let mut open: Vec<(String, usize)> = Vec::new();
    for (at, _) in before.match_indices('<') {
        let rest = &before[at + 1..];
        let (closes, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let name: String = rest.chars().take_while(|ch| in_tag_name(*ch)).collect();
        // A tag still being written closes and opens nothing yet.
        let Some(end) = rest.find('>').filter(|_| !name.is_empty()) else {
            continue;
        };
        let same = |(other, _): &(String, usize)| other.eq_ignore_ascii_case(&name);
        if closes {
            // What was left open inside it is closed with it.
            if let Some(found) = open.iter().rposition(same) {
                open.truncate(found);
            }
        } else if !rest[..end].ends_with('/')
            && !crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str())
        {
            open.push((name, at));
        }
    }
    open.pop()
}

/// Replace the word ending at `caret` by `completion`. Returns the new caret.
pub fn apply(text: &mut String, caret: usize, completion: &Completion, unit: &str) -> usize {
    let start = word_start(text, caret);
    write_snippet(text, start..caret, &completion.body, unit)
}

/// The body of the snippet `label` that comes with `language`.
pub(crate) fn builtin(language: &str, label: &str) -> Option<&'static str> {
    let (_, body) = snippets_of(language).find(|(found, _)| *found == label)?;
    Some(body)
}

/// Write the snippet `body` over the chars `chars`: a tab in it is one `unit` of indent,
/// its lines continue the indent of the line it starts on. The caret goes to its first tab
/// stop, or to `$0`, or to its end. Returns the caret.
pub fn write_snippet(text: &mut String, chars: Range<usize>, body: &str, unit: &str) -> usize {
    write_stops(text, chars, body, unit).first().start
}

/// Write the snippet `body` over the chars `chars`, as `write_snippet` does. Returns what
/// was written, with its tab stops where they now are in `text`.
pub fn write_stops(text: &mut String, chars: Range<usize>, body: &str, unit: &str) -> Expanded {
    let bytes = byte_index(text, chars.start)..byte_index(text, chars.end);
    let line_start = text[..bytes.start].rfind('\n').map_or(0, |index| index + 1);
    let lead: String = text[line_start..]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let mut written = expand(body, unit, &lead);
    text.replace_range(bytes, &written.text);
    let moved = |place: &mut Range<usize>| {
        *place = place.start + chars.start..place.end + chars.start;
    };
    let places = written.stops.iter_mut().flat_map(|stop| &mut stop.ranges);
    places.for_each(moved);
    moved(&mut written.end);
    written
}

/// A tab stop of a snippet: its number, and the chars it takes in the text, in order. More
/// than one range: the same is to be written in each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub number: u32,
    pub ranges: Vec<Range<usize>>,
}

/// A snippet made into text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// The stops `$1`, `$2`… by number: where Tab goes, one after the other.
    pub stops: Vec<Stop>,
    /// Where Tab goes last, in chars: `$0`, or the end of the text.
    pub end: Range<usize>,
}

impl Expanded {
    /// What to select once the snippet is written: its first stop, or its last place.
    pub fn first(&self) -> Range<usize> {
        let first = self.stops.first().and_then(|stop| stop.ranges.first());
        first.unwrap_or(&self.end).clone()
    }

    /// Something was typed in the stop being filled, `stops[step]`, in its first range:
    /// in `text`, `added` chars now stand at the char `at` where `removed` were. Every
    /// place is moved to where it now is, and the other ranges of that stop are given
    /// the same content. `false` when that was typed outside the stop: the snippet is
    /// left, forget it.
    pub fn follow(
        &mut self,
        text: &mut String,
        step: usize,
        at: usize,
        removed: usize,
        added: usize,
    ) -> bool {
        let filled = self.stops.get(step).and_then(|stop| stop.ranges.first());
        let Some(filled) = filled.cloned() else {
            return false;
        };
        if at < filled.start || at + removed > filled.end {
            return false;
        }
        let grown = filled.start..filled.end - removed + added;
        // Of two empty places at the same char, the one of the later stop comes after.
        let ends_before = |place: &Range<usize>, start: usize, earlier: bool| {
            place.end < start || (place.end == start && (earlier || !place.is_empty()))
        };
        self.moved(at..at + removed, added, |index, place| {
            ends_before(place, filled.start, index < step)
        });
        self.stops[step].ranges[0] = grown.clone();
        let from = byte_index(text, grown.start);
        let content = text[from..from + byte_index(&text[from..], grown.len())].to_owned();
        for twin in 1..self.stops[step].ranges.len() {
            let old = self.stops[step].ranges[twin].clone();
            if old.start < grown.end {
                continue;
            }
            let bytes = byte_index(text, old.start)..byte_index(text, old.end);
            text.replace_range(bytes, &content);
            self.moved(old.clone(), grown.len(), |index, place| {
                ends_before(place, old.start, index <= step)
            });
            self.stops[step].ranges[twin] = old.start..old.start + grown.len();
        }
        true
    }

    /// The chars `edited` became `added` chars: each place that is not `before` them, by
    /// the index of its stop, follows.
    fn moved(
        &mut self,
        edited: Range<usize>,
        added: usize,
        before: impl Fn(usize, &Range<usize>) -> bool,
    ) {
        let after = |at: usize| match at >= edited.end {
            true => at - edited.len() + added,
            false => at.min(edited.start),
        };
        let last = self.stops.len();
        let stops = self.stops.iter_mut().enumerate();
        let places = stops.flat_map(|(index, stop)| {
            let ranges = stop.ranges.iter_mut();
            ranges.map(move |place| (index, place))
        });
        for (index, place) in places.chain([(last, &mut self.end)]) {
            if !before(index, place) {
                *place = after(place.start)..after(place.end).max(after(place.start));
            }
        }
    }
}

/// The names VS Code gives to what it knows of the file and the day. None is known here.
const VARIABLES: [&str; 38] = [
    "TM_SELECTED_TEXT",
    "TM_CURRENT_LINE",
    "TM_CURRENT_WORD",
    "TM_LINE_INDEX",
    "TM_LINE_NUMBER",
    "TM_FILENAME",
    "TM_FILENAME_BASE",
    "TM_DIRECTORY",
    "TM_FILEPATH",
    "RELATIVE_FILEPATH",
    "CLIPBOARD",
    "WORKSPACE_NAME",
    "WORKSPACE_FOLDER",
    "CURSOR_INDEX",
    "CURSOR_NUMBER",
    "CURRENT_YEAR",
    "CURRENT_YEAR_SHORT",
    "CURRENT_MONTH",
    "CURRENT_MONTH_NAME",
    "CURRENT_MONTH_NAME_SHORT",
    "CURRENT_DATE",
    "CURRENT_DAY_NAME",
    "CURRENT_DAY_NAME_SHORT",
    "CURRENT_HOUR",
    "CURRENT_MINUTE",
    "CURRENT_SECOND",
    "CURRENT_SECONDS_UNIX",
    "CURRENT_TIMEZONE_OFFSET",
    "RANDOM",
    "RANDOM_HEX",
    "UUID",
    "BLOCK_COMMENT_START",
    "BLOCK_COMMENT_END",
    "LINE_COMMENT",
    "TM_SOFT_TABS",
    "TM_TAB_SIZE",
    "TM_SCOPE",
    "TM_SELECTION",
];

/// Placeholders inside placeholders, no deeper than this: the rest is plain text.
const MAX_NESTING: usize = 16;

/// A part of the body of a snippet.
enum Piece {
    Text(String),
    /// A tab stop, and what it holds until something is typed there.
    Stop(u32, Vec<Piece>),
}

/// The parts of `body` from the char `at` on: to its end, or to the `}` that closes the
/// placeholder they are `inside`.
fn pieces(body: &[char], at: &mut usize, inside: usize) -> Vec<Piece> {
    let mut found = Vec::new();
    let mut plain = String::new();
    while let Some(&ch) = body.get(*at) {
        *at += 1;
        match (ch, body.get(*at)) {
            ('\\', Some(&escaped @ ('$' | '}' | '\\'))) => {
                plain.push(escaped);
                *at += 1;
            }
            ('}', _) if inside > 0 => break,
            ('$', _) => match dollar(body, at, inside) {
                Some(made) => {
                    found.push(Piece::Text(std::mem::take(&mut plain)));
                    found.extend(made);
                }
                None => plain.push('$'),
            },
            _ => plain.push(ch),
        }
    }
    found.push(Piece::Text(plain));
    found
}

/// What the `$` before the char `at` of `body` stands for: a tab stop (`$1`, `${1}`,
/// `${1:text}`, `${1|one,other|}`, `${1/…/…/}`), or nothing but its default for a
/// variable (`$TM_FILENAME`, `${CLIPBOARD:text}`). `None` when it is a plain `$`.
fn dollar(body: &[char], at: &mut usize, inside: usize) -> Option<Vec<Piece>> {
    let braced = body.get(*at) == Some(&'{');
    let start = *at + usize::from(braced);
    // A number goes as far as its digits, a name as far as it reads.
    let numbered = body.get(start).is_some_and(char::is_ascii_digit);
    let is_name = |ch: &char| match numbered {
        true => ch.is_ascii_digit(),
        false => ch.is_ascii_alphanumeric() || *ch == '_',
    };
    let name: String = body[start..].iter().take_while(|ch| is_name(ch)).collect();
    let number = match numbered {
        true => Some(name.parse::<u32>().ok()?),
        false if VARIABLES.contains(&name.as_str()) => None,
        false => return None,
    };
    let mut to = start + name.len();
    let mut held = Vec::new();
    if braced {
        to += 1;
        match body.get(to - 1)? {
            '}' => {}
            ':' if inside < MAX_NESTING => held = pieces(body, &mut to, inside + 1),
            '|' if number.is_some() => held.push(Piece::Text(first_choice(body, &mut to)?)),
            // What rewrites the text of a stop is not done: the text stays as it is.
            '/' => {
                let mut open = 0;
                loop {
                    to += 1;
                    match body.get(to - 1)? {
                        '\\' => to += 1,
                        '{' => open += 1,
                        '}' if open == 0 => break,
                        '}' => open -= 1,
                        _ => {}
                    }
                }
            }
            _ => return None,
        }
    }
    *at = to;
    Some(match number {
        Some(number) => vec![Piece::Stop(number, held)],
        None => held,
    })
}

/// The first of the choices `one,other|}` that start at the char `at` of `body`.
fn first_choice(body: &[char], at: &mut usize) -> Option<String> {
    let mut first = String::new();
    let mut ended = false;
    loop {
        *at += 1;
        match (body.get(*at - 1)?, body.get(*at)) {
            ('\\', Some(escaped)) => {
                if !ended {
                    first.push(*escaped);
                }
                *at += 1;
            }
            ('|', Some('}')) => break,
            (',', _) => ended = true,
            (ch, _) if !ended => first.push(*ch),
            _ => {}
        }
    }
    *at += 1;
    Some(first)
}

/// What the stop `number` holds where it is first given something.
fn held_by(all: &[Piece], number: u32) -> Option<&[Piece]> {
    all.iter().find_map(|piece| match piece {
        Piece::Stop(found, held) if *found == number && !is_empty(held) => Some(held.as_slice()),
        Piece::Stop(_, held) => held_by(held, number),
        Piece::Text(_) => None,
    })
}

fn is_empty(pieces: &[Piece]) -> bool {
    pieces.iter().all(|piece| match piece {
        Piece::Text(text) => text.is_empty(),
        Piece::Stop(..) => false,
    })
}

/// The text a snippet becomes, as it is written.
struct Writer<'a> {
    unit: &'a str,
    lead: &'a str,
    all: &'a [Piece],
    text: String,
    /// The chars of `text`.
    chars: usize,
    stops: Vec<Stop>,
    end: Option<Range<usize>>,
}

impl Writer<'_> {
    /// Write `pieces`. `marked`: their stops are stops; if not, only their text is kept.
    fn write(&mut self, pieces: &[Piece], marked: bool) {
        for piece in pieces {
            match piece {
                Piece::Text(text) => text.chars().for_each(|ch| self.push(ch)),
                Piece::Stop(number, held) => {
                    let start = self.chars;
                    match is_empty(held) {
                        false => self.write(held, marked),
                        // A stop written again shows what it holds the first time.
                        true if marked => {
                            let all = self.all;
                            self.write(held_by(all, *number).unwrap_or_default(), false);
                        }
                        true => {}
                    }
                    if marked {
                        self.mark(*number, start..self.chars);
                    }
                }
            }
        }
    }

    fn push(&mut self, ch: char) {
        let written = match ch {
            '\t' => self.unit,
            '\n' => {
                self.text.push('\n');
                self.chars += 1;
                self.lead
            }
            _ => {
                self.text.push(ch);
                self.chars += 1;
                return;
            }
        };
        self.text.push_str(written);
        self.chars += written.chars().count();
    }

    fn mark(&mut self, number: u32, place: Range<usize>) {
        if number == 0 {
            self.end.get_or_insert(place);
            return;
        }
        match self.stops.iter_mut().find(|stop| stop.number == number) {
            Some(stop) => stop.ranges.push(place),
            None => self.stops.push(Stop {
                number,
                ranges: vec![place],
            }),
        }
    }
}

/// Make the body of a snippet into the text to write: a tab in it is one `unit` of
/// indent, each of its lines after the first starts with `lead`. Its tab stops are those
/// of VS Code: `$1`, `${1:text}`, the same number again for the same text elsewhere,
/// `${1|one,other|}` (the first choice is written), `$0` for the last place; `\$`, `\}`
/// and `\\` are the plain chars. What VS Code knows by name (`$TM_FILENAME`…) is written
/// as nothing, or as its default (`${CLIPBOARD:text}`); any other `$name` is plain text.
pub fn expand(body: &str, unit: &str, lead: &str) -> Expanded {
    let body: Vec<char> = body.chars().collect();
    let all = pieces(&body, &mut 0, 0);
    let mut writer = Writer {
        unit,
        lead,
        all: &all,
        text: String::new(),
        chars: 0,
        stops: Vec::new(),
        end: None,
    };
    writer.write(&all, true);
    writer.stops.sort_by_key(|stop| stop.number);
    Expanded {
        end: writer.end.unwrap_or(writer.chars..writer.chars),
        text: writer.text,
        stops: writer.stops,
    }
}

/// `text` as the body that writes it as it is: nothing in it is a tab stop.
fn literal(text: &str) -> String {
    let mut body = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '$' | '}' | '\\') {
            body.push('\\');
        }
        body.push(ch);
    }
    body
}

/// How what is typed is found in a candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// The higher the better: the candidate is what is typed, then starts with it, then
    /// has its letters where its words start, then has them anywhere.
    pub score: i32,
    /// The chars of the candidate that are those typed, in order.
    pub positions: Vec<usize>,
}

const EXACT: i32 = 40_000;
const PREFIX: i32 = 30_000;
const WORD_STARTS: i32 = 20_000;
const SCATTERED: i32 = 10_000;
/// What a letter is worth where the candidate starts, where one of its words does, and
/// right after the letter before it.
const AT_START: i32 = 100;
const AT_WORD: i32 = 80;
const NEXT: i32 = 60;
/// What a letter typed in the case it is written in is worth more.
const SAME_CASE: i32 = 5;
/// What each char stepped over costs.
const GAP: i32 = 1;
const NO_SCORE: i32 = -1_000_000;
/// The chars of a candidate that are looked at.
const MAX_LOOKED: usize = 64;

fn lowered(ch: char) -> char {
    match ch.is_ascii() {
        true => ch.to_ascii_lowercase(),
        false => ch.to_lowercase().next().unwrap_or(ch),
    }
}

/// What is typed, ready to be looked for in many candidates.
struct Needle {
    typed: Vec<char>,
    lower: Vec<char>,
    /// `lower` in bytes, when it is all ASCII.
    ascii: Option<Vec<u8>>,
    /// Of the candidate being looked at: its chars, the same lowered, and what a letter
    /// is worth at each.
    looked: Vec<char>,
    low: Vec<char>,
    worth: Vec<i32>,
    /// By letter typed and char of the candidate: the best score with the letter at that
    /// char, the best with it there or before, and whether the first follows the letter
    /// before at the char before.
    ending: Vec<i32>,
    so_far: Vec<i32>,
    joined: Vec<bool>,
    /// Where the letters were last found.
    at: Vec<usize>,
}

impl Needle {
    fn new(typed: &str) -> Self {
        let lower: Vec<char> = typed.chars().map(lowered).collect();
        let ascii = typed
            .is_ascii()
            .then(|| typed.to_ascii_lowercase().into_bytes());
        Self {
            typed: typed.chars().collect(),
            lower,
            ascii,
            looked: Vec::new(),
            low: Vec::new(),
            worth: Vec::new(),
            ending: Vec::new(),
            so_far: Vec::new(),
            joined: Vec::new(),
            at: Vec::new(),
        }
    }

    /// Whether the letters typed are in `word`, in order. Cheap: no score is made.
    fn is_in(&self, word: &str) -> bool {
        let Some(ascii) = &self.ascii else {
            let mut letters = self.lower.iter().peekable();
            for ch in word.chars() {
                letters.next_if(|letter| **letter == lowered(ch));
            }
            return letters.peek().is_none();
        };
        let mut found = 0;
        for byte in word.bytes() {
            if ascii.get(found) == Some(&byte.to_ascii_lowercase()) {
                found += 1;
            }
        }
        found == ascii.len()
    }

    /// How well `candidate` is what is typed; where its letters are is left in `at`.
    fn score(&mut self, candidate: &str) -> Option<i32> {
        let typed = self.lower.len();
        self.at.clear();
        if typed == 0 {
            return Some(0);
        }
        self.looked.clear();
        self.looked.extend(candidate.chars().take(MAX_LOOKED));
        let length = self.looked.len();
        if typed > length {
            return None;
        }
        let same_case = |letter: usize, ch: char| match ch == self.typed[letter] {
            true => SAME_CASE,
            false => 0,
        };
        let starts = (0..typed).all(|letter| lowered(self.looked[letter]) == self.lower[letter]);
        if starts {
            let cased: i32 = (0..typed).map(|at| same_case(at, self.looked[at])).sum();
            self.at.extend(0..typed);
            let whole = candidate.chars().nth(typed).is_none();
            return Some(if whole { EXACT } else { PREFIX } + cased);
        }

        // A word starts after what is no letter, and at a capital after a small letter.
        self.worth.clear();
        let mut before = None::<char>;
        for ch in &self.looked {
            self.worth.push(match before {
                None => AT_START,
                Some(before) if !before.is_alphanumeric() && ch.is_alphanumeric() => AT_WORD,
                Some(before) if before.is_lowercase() && ch.is_uppercase() => AT_WORD,
                Some(_) => 0,
            });
            before = Some(*ch);
        }
        self.low.clear();
        self.low.extend(self.looked.iter().map(|ch| lowered(*ch)));
        let cells = typed * length;
        for scores in [&mut self.ending, &mut self.so_far] {
            scores.clear();
            scores.resize(cells, NO_SCORE);
        }
        self.joined.clear();
        self.joined.resize(cells, false);
        for letter in 0..typed {
            let mut best = NO_SCORE;
            for at in letter..length {
                let cell = letter * length + at;
                if self.low[at] == self.lower[letter] {
                    let place = self.worth[at] + same_case(letter, self.looked[at]);
                    let (score, joined) = match (letter, at) {
                        (0, _) => (place - at as i32 * GAP, false),
                        _ => {
                            let apart = self.so_far[cell - length - 1] + place;
                            let next = self.ending[cell - length - 1] + NEXT.max(place);
                            (apart.max(next), next >= apart)
                        }
                    };
                    self.ending[cell] = score;
                    self.joined[cell] = joined;
                }
                // What follows the last letter costs nothing: length is counted apart.
                let gap = if letter + 1 == typed { 0 } else { GAP };
                best = self.ending[cell].max(best - gap);
                self.so_far[cell] = best;
            }
        }
        let best = self.so_far[cells - 1];
        if best < NO_SCORE / 2 {
            return None;
        }
        // Back from the last letter: each is where its score was made.
        self.at.resize(typed, 0);
        let (mut at, mut forced) = (length - 1, false);
        for letter in (0..typed).rev() {
            let mut cell = letter * length + at;
            while !(forced || self.ending[cell] == self.so_far[cell]) && at > letter {
                at -= 1;
                cell -= 1;
            }
            forced = self.joined[cell];
            self.at[letter] = at;
            at = at.saturating_sub(1);
        }
        let at = &self.at;
        let by_words = (0..typed).all(|letter| {
            self.worth[at[letter]] > 0 || (letter > 0 && at[letter] == at[letter - 1] + 1)
        });
        let class = if by_words { WORD_STARTS } else { SCATTERED };
        Some(class + best.clamp(-5_000, 5_000))
    }

    fn matched(&mut self, candidate: &str) -> Option<Match> {
        let score = self.score(candidate)?;
        Some(Match {
            score,
            positions: self.at.clone(),
        })
    }
}

/// How `typed` is found in `candidate`: its letters are there in that order, whatever
/// their case (`gtb` in `getTextBounds`). `None` when they are not.
pub fn fuzzy(typed: &str, candidate: &str) -> Option<Match> {
    Needle::new(typed).matched(candidate)
}

/// Those of `candidates` that `typed` is found in, best first, each by its index: the
/// better score, then the shorter, then the first given.
pub fn rank(typed: &str, candidates: &[&str]) -> Vec<(usize, Match)> {
    let mut needle = Needle::new(typed);
    let found = candidates.iter().enumerate();
    let mut found: Vec<(usize, Match)> = found
        .filter_map(|(index, candidate)| Some((index, needle.matched(candidate)?)))
        .collect();
    found.sort_by_key(|(index, found)| {
        let length = candidates[*index].chars().count();
        (std::cmp::Reverse(found.score), length)
    });
    found
}

/// Where a suggestion comes from. In that order when they match as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// A name written after a `.` or a `::` somewhere in the files.
    Member,
    /// A file or a folder.
    Path,
    Snippet,
    Keyword,
    /// A name the language comes with: a type, a function, a macro.
    Builtin,
    /// A word of the file.
    Word,
    /// A word of another open file.
    Other,
}

impl Source {
    /// The kind a completion of that source is shown as.
    pub fn kind(self) -> Kind {
        match self {
            Self::Snippet => Kind::Snippet,
            Self::Keyword | Self::Builtin => Kind::Keyword,
            Self::Member | Self::Path | Self::Word | Self::Other => Kind::Word,
        }
    }
}

/// What to complete, and with what.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub text: &'a str,
    /// The char the caret is at.
    pub caret: usize,
    pub language: &'a Language,
    /// The snippets of the user for this language: what is typed, what it becomes.
    pub snippets: &'a [(String, String)],
    /// The texts of the other open files.
    pub others: &'a [&'a str],
    /// The files around the one edited: their paths from its folder, with `/`
    /// (`lib/math.js`, `../shared/api.js`).
    pub files: &'a [&'a str],
}

impl<'a> Request<'a> {
    /// A request with nothing but the file itself to look in.
    pub fn new(text: &'a str, caret: usize, language: &'a Language) -> Self {
        Self {
            text,
            caret,
            language,
            snippets: &[],
            others: &[],
            files: &[],
        }
    }
}

/// Something the caret could go on with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    /// What is written: the body of a snippet, tab stops and all.
    pub body: String,
    pub kind: Kind,
    pub source: Source,
    /// The chars of the label that are those typed.
    pub matched: Vec<usize>,
    /// The chars of the text the body is written over: what was typed of it.
    pub replace: Range<usize>,
}

impl Suggestion {
    /// The same as a completion, for what takes one.
    pub fn completion(&self) -> Completion {
        completion(&self.label, &self.body, self.kind)
    }
}

/// Write `suggestion` in `text` over what was typed of it. Returns what was written: where
/// to put the caret (`first`), and the tab stops to walk.
pub fn accept(text: &mut String, suggestion: &Suggestion, unit: &str) -> Expanded {
    write_stops(text, suggestion.replace.clone(), &suggestion.body, unit)
}

/// A word of a text: how many lines from the caret it is the nearest, anywhere and
/// after the separator looked for (`usize::MAX` where it never is).
struct Sighted<'a> {
    word: &'a str,
    near: usize,
    after: usize,
}

/// Whether a byte is of a word: any byte beyond ASCII may be, the chars tell.
const IN_WORD: [bool; 256] = {
    let mut table = [false; 256];
    let mut byte = 0;
    while byte < 256 {
        table[byte] = (byte as u8).is_ascii_alphanumeric() || byte == b'_' as usize || byte >= 0x80;
        byte += 1;
    }
    table
};

/// A hash for names: quicker on short keys than the one of the standard library.
#[derive(Default)]
struct NameHasher(u64);

impl std::hash::Hasher for NameHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

type Names<'a> = HashMap<&'a str, usize, std::hash::BuildHasherDefault<NameHasher>>;

/// The words of `text` the letters typed are in. `typing`: the byte the word being typed
/// starts at, left out, and the line of the caret; without it every word is as near.
/// `separator`: what comes before a word that makes it a member (`.`, `::`), if any.
fn sighted<'a>(
    text: &'a str,
    needle: &Needle,
    typing: Option<(usize, usize)>,
    separator: &str,
) -> Vec<Sighted<'a>> {
    let bytes = text.as_bytes();
    let mut found: Vec<Sighted<'a>> = Vec::new();
    let mut known = Names::default();
    // `fits`: the letters typed are known to be in the word.
    let mut note = |word: Range<usize>, line: usize, fits: bool| {
        let start = word.start;
        let word = &text[word];
        if word.starts_with(|ch: char| ch.is_ascii_digit()) || !(fits || needle.is_in(word)) {
            return;
        }
        let before = &text[..start];
        let follows = !separator.is_empty()
            && before.ends_with(separator)
            && !(separator == "." && before.ends_with(".."));
        let length = word.chars().take(MIN_WORD_CHARS).count();
        if typing.is_some_and(|(typed, _)| typed == start) || length < MIN_MEMBER_CHARS {
            return;
        }
        let far = typing.map_or(0, |(_, caret)| line.abs_diff(caret));
        // With nothing typed, only members are of use.
        let near = match length < MIN_WORD_CHARS || needle.lower.is_empty() {
            true => usize::MAX,
            false => far,
        };
        let after = if follows { far } else { usize::MAX };
        if near.min(after) == usize::MAX {
            return;
        }
        let index = *known.entry(word).or_insert(found.len());
        match found.get_mut(index) {
            Some(seen) => (seen.near, seen.after) = (seen.near.min(near), seen.after.min(after)),
            None => found.push(Sighted { word, near, after }),
        }
    };
    // Letters of ASCII are looked for while the word is read: most words are left there.
    let letters = needle.ascii.as_deref();
    let (mut at, mut line) = (0, 0);
    while at < bytes.len() {
        if !IN_WORD[usize::from(bytes[at])] {
            line += usize::from(bytes[at] == b'\n');
            at += 1;
            continue;
        }
        let start = at;
        let (mut ascii, mut matched) = (true, 0);
        while at < bytes.len() && IN_WORD[usize::from(bytes[at])] {
            let byte = bytes[at];
            ascii &= byte < 0x80;
            let wanted = letters.and_then(|letters| letters.get(matched));
            matched += usize::from(wanted == Some(&byte.to_ascii_lowercase()));
            at += 1;
        }
        if ascii {
            match letters {
                Some(letters) if matched < letters.len() => {}
                letters => note(start..at, line, letters.is_some()),
            }
            continue;
        }
        // Not every char beyond ASCII is of a word: a dash, a quote.
        let mut from = start;
        for (offset, ch) in text[start..at].char_indices() {
            if !is_word_char(ch) {
                note(from..start + offset, line, false);
                from = start + offset + ch.len_utf8();
            }
        }
        note(from..at, line, false);
    }
    found
}

/// Members are worth offering shorter than words are: `id`, `ok`.
const MIN_MEMBER_CHARS: usize = 2;

/// A suggestion in the making, with what it is ranked by.
struct Candidate<'a> {
    label: &'a str,
    body: &'a str,
    source: Source,
    /// Those before any other: members after a dot, the snippet typed whole.
    first: bool,
    score: i32,
    /// In lines from the caret.
    distance: usize,
}

/// What the caret could go on with, best first: the snippet named by what is typed, then
/// what matches best of the snippets, the keywords, the names the language comes with,
/// the words of the file (the nearer the caret first) and those of the other files.
/// After a `.` or a `::`: first the names written after one in the files. In a string
/// that is a path: the files and folders that go there, one step of the path at a time.
/// In markup and style sheets: what `complete` offers.
pub fn suggest(request: &Request<'_>) -> Vec<Suggestion> {
    let text = request.text;
    let end = byte_index(text, request.caret);
    let caret = match end == text.len() {
        true => text.chars().count(),
        false => request.caret,
    };
    let found = paths(request, end, caret);
    if !found.is_empty() {
        return found;
    }
    let word: usize = text[..end]
        .chars()
        .rev()
        .take_while(|ch| is_word_char(*ch))
        .map(char::len_utf8)
        .sum();
    let (before, typed) = text[..end].split_at(end - word);
    let replace = caret - typed.chars().count()..caret;
    let separator = match () {
        () if before.ends_with("::") => "::",
        () if before.ends_with('.') && !before.ends_with("..") => ".",
        () => "",
    };
    let nothing = typed.is_empty() && separator.is_empty();
    if nothing || typed.starts_with(|ch: char| ch.is_ascii_digit()) {
        return Vec::new();
    }
    let suggestion = |label: &str, body: &str, source: Source, matched: Vec<usize>| Suggestion {
        label: label.to_owned(),
        body: body.to_owned(),
        kind: source.kind(),
        source,
        matched,
        replace: replace.clone(),
    };
    let placed = match typed.is_empty() {
        true => Vec::new(),
        false => placed(text, end, typed, request.language),
    };
    if !placed.is_empty() {
        // What is typed of a name starts it, hyphens and all.
        let named = |ch: &char| is_word_char(*ch) || *ch == '-';
        let written = text[..end].chars().rev().take_while(named).count();
        let made = placed.iter().map(|placed| {
            let matched = (0..written.min(placed.label.chars().count())).collect();
            suggestion(&placed.label, &placed.body, Source::Keyword, matched)
        });
        return made.collect();
    }

    let mut needle = Needle::new(typed);
    let mut found: Vec<Candidate<'_>> = Vec::new();
    // A word typed in full has nothing left to complete.
    let mut seen: HashSet<&str> = HashSet::from([typed]);
    let mut offer = |needle: &mut Needle, label, body, source, first, distance| {
        if let Some(score) = needle.score(label) {
            found.push(Candidate {
                label,
                body,
                source,
                first,
                score,
                distance,
            });
        }
    };
    let line = text.as_bytes()[..end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    let own = sighted(text, &needle, Some((end - word, line)), separator);
    let others = request.others.iter();
    let others: Vec<Sighted<'_>> = others
        .flat_map(|other| sighted(other, &needle, None, separator))
        .collect();
    let member = !separator.is_empty();
    if member {
        // Those of the other files after all those of this one.
        let members = own.iter().map(|sighted| (sighted, sighted.after));
        let further = others
            .iter()
            .map(|sighted| (sighted, sighted.after.max(usize::MAX - 1)));
        for (sighted, after) in members.chain(further) {
            if after != usize::MAX && seen.insert(sighted.word) {
                let word = sighted.word;
                offer(&mut needle, word, word, Source::Member, true, after);
            }
        }
    } else {
        let user = request.snippets.iter();
        let mut snippets: Vec<(&str, &str)> = user
            .map(|(label, body)| (label.as_str(), body.as_str()))
            .collect();
        for snippet in stops_snippets_of(request.language.name) {
            snippets.push(snippet);
        }
        let mut labels: HashSet<&str> = HashSet::new();
        // A snippet of the user takes the place of the one that comes under its name.
        for (label, body) in snippets {
            if labels.insert(label) {
                let whole = label.eq_ignore_ascii_case(typed);
                offer(&mut needle, label, body, Source::Snippet, whole, 0);
            }
        }
    }
    // After a dot with nothing typed, members alone: not every word there is.
    if !typed.is_empty() {
        for keyword in request.language.keywords() {
            if seen.insert(keyword) {
                offer(&mut needle, keyword, keyword, Source::Keyword, false, 0);
            }
        }
        let builtins = BUILTINS
            .iter()
            .filter(|(name, _)| *name == request.language.name);
        for builtin in builtins
            .flat_map(|(_, names)| names.iter())
            .filter(|_| !member)
        {
            // A macro is known by its name: `println` in the file is `println!`.
            if seen.insert(builtin.trim_end_matches('!')) {
                seen.insert(builtin);
                offer(&mut needle, builtin, builtin, Source::Builtin, false, 0);
            }
        }
        let own = own.iter().map(|sighted| (sighted, Source::Word));
        for (sighted, source) in own.chain(others.iter().map(|sighted| (sighted, Source::Other))) {
            if sighted.near != usize::MAX && seen.insert(sighted.word) {
                let word = sighted.word;
                offer(&mut needle, word, word, source, false, sighted.near);
            }
        }
    }

    let mut ranked: Vec<(usize, &Candidate<'_>)> = found.iter().enumerate().collect();
    let key = |(order, candidate): &(usize, &Candidate<'_>)| {
        (
            !candidate.first,
            std::cmp::Reverse(candidate.score),
            candidate.source,
            candidate.distance,
            candidate.label.chars().count(),
            *order,
        )
    };
    // Only the best are put in order: there can be as many as words in the file.
    if ranked.len() > MAX_COMPLETIONS {
        ranked.select_nth_unstable_by_key(MAX_COMPLETIONS, key);
        ranked.truncate(MAX_COMPLETIONS);
    }
    ranked.sort_unstable_by_key(key);
    let made = ranked.into_iter().map(|(_, candidate)| {
        let matched = needle.matched(candidate.label);
        let matched = matched.map_or_else(Vec::new, |matched| matched.positions);
        suggestion(candidate.label, candidate.body, candidate.source, matched)
    });
    made.collect()
}

/// What comes before the quote of a string that is a path, whatever the path looks like.
const PATH_OPENERS: [&str; 8] = [
    "from", "import", "require(", "import(", "#include", "src=", "href=", "url(",
];

/// The path being written in a string at the end of `line`, a line up to the caret: one
/// that starts with `./` or `../`, or goes where a path is expected. `None` elsewhere.
fn path_written(line: &str) -> Option<&str> {
    let mut open = None;
    let mut chars = line.char_indices();
    while let Some((at, ch)) = chars.next() {
        match open {
            Some(_) if ch == '\\' => {
                chars.next();
            }
            Some((quote, _)) if ch == quote => open = None,
            None if matches!(ch, '"' | '\'' | '`') => open = Some((ch, at)),
            _ => {}
        }
    }
    let (_, opened) = open?;
    let (before, written) = (line[..opened].trim_end(), &line[opened + 1..]);
    if written.contains(|ch: char| ch.is_whitespace() || ch == ':') {
        return None;
    }
    let expected = PATH_OPENERS.iter().any(|opener| {
        let rest = before.strip_suffix(opener);
        // `from`, not `wherefrom`.
        rest.is_some_and(|rest| !opener.starts_with(is_word_char) || !rest.ends_with(is_word_char))
    });
    (expected || written.starts_with("./") || written.starts_with("../")).then_some(written)
}

/// The name of the module being declared at the end of `line`, a line of Rust up to the
/// caret: what follows `mod`.
fn module_written(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let public = line.strip_prefix("pub(crate) ");
    let line = public.or_else(|| line.strip_prefix("pub ")).unwrap_or(line);
    line.strip_prefix("mod ")
        .filter(|name| name.chars().all(is_word_char))
}

/// The files and folders of the request that go on with the path being written at the
/// byte `end`: the next step of it only. Empty when no path is being written.
fn paths(request: &Request<'_>, end: usize, caret: usize) -> Vec<Suggestion> {
    if request.files.is_empty() {
        return Vec::new();
    }
    let line = &request.text[..end];
    let line = &line[line.rfind('\n').map_or(0, |at| at + 1)..];
    // `./a` is `a`.
    fn plain(path: &str) -> &str {
        path.strip_prefix("./").map_or(path, plain)
    }
    let files = request.files.iter().map(|file| plain(file));
    let module = match request.language.name {
        "Rust" => module_written(line),
        _ => None,
    };
    let (typed, names): (&str, Vec<&str>) = if let Some(typed) = module {
        // A module is a file of Rust beside this one, or a folder that holds `mod.rs`.
        let modules = files.filter_map(|file| match file.split_once('/') {
            None => file.strip_suffix(".rs"),
            Some((folder, "mod.rs")) => Some(folder),
            Some(_) => None,
        });
        let named = modules.filter(|name| !matches!(*name, "mod" | "main" | "lib"));
        (typed, named.collect())
    } else if let Some(written) = path_written(line) {
        let (folder, typed) = written.split_at(written.rfind('/').map_or(0, |at| at + 1));
        let folder = plain(folder);
        let steps = files.filter_map(|file| {
            let rest = file.strip_prefix(folder)?;
            Some(&rest[..rest.find('/').map_or(rest.len(), |at| at + 1)])
        });
        (typed, steps.collect())
    } else {
        return Vec::new();
    };
    let mut seen: HashSet<&str> = HashSet::from(["", typed]);
    let mut needle = Needle::new(typed);
    let mut found: Vec<(&str, Match)> = names
        .into_iter()
        .filter(|name| seen.insert(name))
        .filter_map(|name| Some((name, needle.matched(name)?)))
        .collect();
    found.sort_by_key(|(name, found)| (std::cmp::Reverse(found.score), name.chars().count()));
    found.truncate(MAX_COMPLETIONS);
    let made = found.into_iter().map(|(name, found)| Suggestion {
        label: name.to_owned(),
        body: literal(name),
        kind: Source::Path.kind(),
        source: Source::Path,
        matched: found.positions,
        replace: caret - typed.chars().count()..caret,
    });
    made.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn labels(text: &str, file: &str) -> Vec<(String, Kind)> {
        complete(text, text.chars().count(), Language::of(Path::new(file)))
            .into_iter()
            .map(|completion| (completion.label, completion.kind))
            .collect()
    }

    #[test]
    fn snippets_keywords_then_words_are_offered() {
        let found = labels("let matcher = 1;\nlet mate = 2;\nma", "a.rs");
        assert_eq!(
            found,
            [
                ("match".to_owned(), Kind::Snippet),
                ("main".to_owned(), Kind::Snippet),
                ("match".to_owned(), Kind::Keyword),
                ("mate".to_owned(), Kind::Word),
                ("matcher".to_owned(), Kind::Word),
            ]
        );
    }

    #[test]
    fn a_word_typed_in_full_still_offers_the_structure_it_names() {
        // `main` typed whole: its snippet is the first, and writes the function.
        for (file, opening) in [
            ("a.rs", "fn main() {"),
            ("a.py", "def main():"),
            ("a.go", "func main() {"),
            ("a.c", "int main("),
            ("a.java", "public static void main("),
            ("a.js", "function main() {"),
        ] {
            let language = Language::of(Path::new(file));
            let found = complete("main", 4, language);
            let first = found.first().expect(file);
            assert_eq!(
                (first.label.as_str(), first.kind),
                ("main", Kind::Snippet),
                "{file}"
            );
            assert!(first.body.contains(opening), "{file}: {}", first.body);
            let mut text = "main".to_owned();
            let caret = apply(&mut text, 4, first, "    ");
            assert!(
                text.contains(opening) && !text.contains("$0"),
                "{file}: {text}"
            );
            assert!(caret <= text.chars().count(), "{file}");
        }
        // Of two that fit, the one typed whole is first.
        let found = complete("if", 2, Language::of(Path::new("a.rs")));
        let labels: Vec<&str> = found.iter().map(|found| found.label.as_str()).collect();
        assert_eq!(labels[..3], ["if", "iflet", "ifelse"]);
        // TypeScript has what JavaScript has, and its own.
        let typed = complete("inter", 5, Language::of(Path::new("a.ts")));
        assert_eq!(
            typed.first().map(|found| found.label.as_str()),
            Some("interface")
        );
    }

    #[test]
    fn nothing_is_offered_without_a_word_or_for_a_finished_one() {
        assert!(labels("total = 1\n", "a.py").is_empty());
        assert!(labels("total = 12", "a.py").is_empty());
        assert!(labels("total = 1\ntotal", "a.py").is_empty());
        assert_eq!(
            labels("étage = 1\nét", "a.py"),
            [("étage".to_owned(), Kind::Word)]
        );
    }

    #[test]
    fn a_snippet_is_indented_and_places_the_caret() {
        let mut text = "class A:\n    de".to_owned();
        let found = complete(&text, 15, Language::of(Path::new("a.py")));
        assert_eq!(found[0].label, "def");
        let caret = apply(&mut text, 15, &found[0], "    ");
        assert_eq!(text, "class A:\n    def ():\n        pass");
        assert_eq!(caret, 17);
    }

    #[test]
    fn an_opening_tag_gets_its_closing_tag() {
        assert_eq!(
            closing_tag("<div class=\"a\"", "").as_deref(),
            Some("</div>")
        );
        assert_eq!(closing_tag("<p>text <b", "</p>").as_deref(), Some("</b>"));
        let none = [
            ("<br", ""),
            ("</div", ""),
            ("<img /", ""),
            ("a ", ""),
            ("<b", "</b>"),
        ];
        for (before, after) in none {
            assert_eq!(closing_tag(before, after), None, "{before}");
        }
        // A closing tag already there, further down, is the one: no second one is written.
        assert_eq!(
            closing_tag("<html>\n<body", "\n<p>a</p>\n</body>\n</html>"),
            None
        );
        assert_eq!(closing_tag("<BODY", "\n</body>"), None);
        // One that closes an element opened before is not for this tag.
        assert_eq!(
            closing_tag("<div>\n  <div", "\n</div>").as_deref(),
            Some("</div>")
        );
        assert_eq!(closing_tag("<div>\n  <div", "\n  </div>\n</div>"), None);
        // Nor is one that closes an element opened after it.
        assert_eq!(closing_tag("<p", "<p>a</p>").as_deref(), Some("</p>"));
        // A name that only starts the same is another element.
        assert_eq!(closing_tag("<b", "</body>").as_deref(), Some("</b>"));
    }

    #[test]
    fn attributes_and_styles_are_offered_where_they_go() {
        let offered = |text: &str, file: &str| {
            let found = complete(text, text.chars().count(), Language::of(Path::new(file)));
            found
                .into_iter()
                .map(|completion| completion.body)
                .collect::<Vec<_>>()
        };
        // In a tag: the attributes of its element, then those of any element.
        assert_eq!(offered("<a h", "a.html"), ["href=\"$0\"", "hidden"]);
        assert_eq!(offered("<input class=\"a\" re", "a.html"), ["required"]);
        assert_eq!(offered("<p cl", "a.html"), ["class=\"$0\""]);
        // Not in the value of one, nor outside a tag.
        assert!(!offered("<p class=\"cl", "a.html").contains(&"class=\"$0\"".to_owned()));
        assert!(!offered("<p>cl", "a.html").contains(&"class=\"$0\"".to_owned()));
        // In a style sheet: properties in a block, values after a property.
        assert_eq!(offered("a {\n  disp", "a.css"), ["display: $0;"]);
        assert_eq!(offered("a {\n  display: fl", "a.css"), ["flex"]);
        assert_eq!(offered("a {\n  color: in", "a.css"), ["inherit", "initial"]);
        assert!(!offered("disp", "a.css").contains(&"display: $0;".to_owned()));
        // A name with hyphens is offered whole and written from the word on.
        assert_eq!(
            offered("a {\n  font-s", "a.css"),
            ["size: $0;", "style: $0;"]
        );
        assert_eq!(offered("a { display: inline-b", "a.css"), ["block"]);
        assert_eq!(offered("<p aria-l", "a.html"), ["label=\"$0\""]);
        // In a page: the style element is a style sheet, the style attribute a block.
        assert_eq!(
            offered("<style>\nbody {\n  marg", "a.html"),
            [
                "margin: $0;",
                "margin-bottom: $0;",
                "margin-left: $0;",
                "margin-right: $0;",
                "margin-top: $0;"
            ]
        );
        assert_eq!(
            offered("<p style=\"color: red; disp", "a.html"),
            ["display: $0;"]
        );
        assert_eq!(offered("<p style=\"display: gr", "a.html"), ["grid"]);
        assert!(
            !offered("<style>a{}</style>\n<p>disp", "a.html").contains(&"display: $0;".to_owned())
        );
        // The values of an attribute, between its quotes.
        assert_eq!(offered("<input type=\"pa", "a.html"), ["password"]);
        assert_eq!(offered("<a target=\"_b", "a.html"), ["_blank"]);
    }

    #[test]
    fn the_tag_that_goes_with_an_edited_one_is_found() {
        let twin = |text: &str, caret: usize| twin_tag(text, caret);
        // A char typed in the name of the opening tag: the closing one takes the name.
        assert_eq!(
            twin("<divx><p>a</p></div>", 5),
            Some((16..19, "divx".to_owned()))
        );
        // A char erased in the closing tag: the opening one follows.
        assert_eq!(
            twin("<div><br>a<img/></di>", 20),
            Some((1..4, "di".to_owned()))
        );
        // Down to no name at all, and back.
        assert_eq!(twin("<><b>x</b></p>", 1), Some((12..13, String::new())));
        assert_eq!(twin("<h><b>x</b></>", 2), Some((13..13, "h".to_owned())));
        // Tags that did not go together are left alone: so are those that already do.
        assert_eq!(twin("<div>a</span>", 3), None);
        assert_eq!(twin("<div>a</div>", 3), None);
        assert_eq!(twin("<br>a", 2), None);
        assert_eq!(twin("a < b", 3), None);
        assert_eq!(twin("<div>a</div>", 5), None);
        // A tag still being typed is the start of a new element, not the old one renamed.
        assert_eq!(
            twin(
                "<div>
<di
</div>",
                9
            ),
            None
        );
        assert_eq!(
            twin(
                "<div>
<di 
</div>",
                9
            ),
            None
        );
        assert!(one_char_apart("div", "dv") && one_char_apart("", "a"));
        assert!(!one_char_apart("div", "div") && !one_char_apart("div", "span"));
    }

    #[test]
    fn the_element_still_open_is_found() {
        let text = "<html>\n  <body>\n    <p>a <b>b</b><br></p>\n    <img src=\"x\" />\n    ";
        assert_eq!(unclosed_tag(text), Some(("body".to_owned(), 9)));
        assert_eq!(unclosed_tag("<ul><li>a"), Some(("li".to_owned(), 4)));
        // A closing tag closes what was left open inside its element too.
        assert_eq!(unclosed_tag("<ul><li>a</ul>"), None);
        assert_eq!(unclosed_tag("<!doctype html>\n"), None);
        assert_eq!(unclosed_tag("<div"), None);
    }

    #[test]
    fn a_word_replaces_what_was_typed() {
        let mut text = "counter = 0\ncou + 1".to_owned();
        let found = complete(&text, 15, Language::of(Path::new("a.py")));
        let caret = apply(&mut text, 15, &found[0], "    ");
        assert_eq!(text, "counter = 0\ncounter + 1");
        assert_eq!(caret, 19);
    }

    /// What is suggested at the end of `text`, a file named `file`.
    fn suggested(text: &str, file: &str) -> Vec<Suggestion> {
        let language = Language::of(Path::new(file));
        suggest(&Request::new(text, text.chars().count(), language))
    }

    /// The labels of those of `found` that come from `source`, in order.
    fn from(found: &[Suggestion], source: Source) -> Vec<&str> {
        let own = found.iter().filter(|found| found.source == source);
        own.map(|found| found.label.as_str()).collect()
    }

    fn place(found: &[Suggestion], label: &str) -> usize {
        let at = found.iter().position(|found| found.label == label);
        at.unwrap_or_else(|| panic!("{label} is not suggested"))
    }

    #[test]
    fn what_is_typed_is_found_by_its_letters_in_order() {
        let positions =
            |typed: &str, candidate: &str| fuzzy(typed, candidate).map(|found| found.positions);
        assert_eq!(positions("gtb", "getTextBounds"), Some(vec![0, 3, 7]));
        assert_eq!(positions("fmn", "fn main"), Some(vec![0, 3, 6]));
        // Where a word starts rather than in the middle of one.
        assert_eq!(positions("gtb", "get_text_bounds"), Some(vec![0, 4, 9]));
        assert_eq!(positions("text", "getTextBounds"), Some(vec![3, 4, 5, 6]));
        assert_eq!(positions("get", "getTextBounds"), Some(vec![0, 1, 2]));
        assert_eq!(positions("GET", "getTextBounds"), Some(vec![0, 1, 2]));
        assert_eq!(positions("ét", "Étage"), Some(vec![0, 1]));
        assert_eq!(positions("", "anything"), Some(Vec::new()));
        // Not in that order, not all there, more than there is.
        assert_eq!(positions("gbt", "getTextBounds"), None);
        assert_eq!(positions("gtz", "getTextBounds"), None);
        assert_eq!(positions("abc", "ab"), None);
    }

    #[test]
    fn the_better_a_match_the_higher_it_ranks() {
        let order = |typed: &str, candidates: &[&str]| {
            let ranked = rank(typed, candidates);
            ranked.iter().map(|(index, _)| *index).collect::<Vec<_>>()
        };
        // The word itself, what it starts, where words start, anywhere; what does not
        // match is left out.
        let candidates = ["xgxtxb", "getTextBounds", "nothing", "gtbar", "gtb"];
        assert_eq!(order("gtb", &candidates), [4, 3, 1, 0]);
        let scores: Vec<i32> = rank("gtb", &candidates)
            .iter()
            .map(|(_, found)| found.score)
            .collect();
        assert!(
            scores.windows(2).all(|pair| pair[0] > pair[1]),
            "{scores:?}"
        );
        // The word itself in another case is still before what it starts.
        assert_eq!(order("foo", &["foobar", "Foo"]), [1, 0]);
        // Where words start beats shorter.
        assert_eq!(order("gt", &["agt", "getText"]), [1, 0]);
        // The case typed beats the other one.
        assert_eq!(order("Foo", &["foobar", "Foobar"]), [1, 0]);
        assert_eq!(order("foo", &["Foobar", "foobar"]), [1, 0]);
        assert_eq!(order("gtb", &["getTextBounds", "get_the_bounds"]), [1, 0]);
        // As good a match: the shorter one, then the first given.
        assert_eq!(order("ab", &["abcdef", "abc"]), [1, 0]);
        assert_eq!(order("ab", &["abz", "abx", "aby"]), [0, 1, 2]);
    }

    #[test]
    fn a_suggestion_tells_which_of_its_letters_were_typed() {
        let found = suggested("const getTextBounds = 1;\ngtb", "a.js");
        let first = &found[place(&found, "getTextBounds")];
        assert_eq!(first.matched, [0, 3, 7]);
        assert_eq!((first.kind, first.source), (Kind::Word, Source::Word));
        // What it is written over: the three letters typed.
        assert_eq!(first.replace, 25..28);
        assert_eq!(first.completion().body, "getTextBounds");
    }

    #[test]
    fn each_source_is_suggested_under_its_own_name() {
        let has = |text: &str, file: &str, label: &str, source: Source, kind: Kind| {
            let found = suggested(text, file);
            let mut own = found.iter().filter(|found| found.source == source);
            let own = own.find(|found| found.label == label);
            assert_eq!(own.map(|found| found.kind), Some(kind), "{label}");
        };
        has("fo", "a.rs", "for", Source::Snippet, Kind::Snippet);
        has("whi", "a.py", "while", Source::Keyword, Kind::Keyword);
        has("isin", "a.py", "isinstance", Source::Builtin, Kind::Keyword);
        has("Has", "a.rs", "HashMap", Source::Builtin, Kind::Keyword);
        has("prom", "a.ts", "Promise", Source::Builtin, Kind::Keyword);
        has("rec", "a.ts", "Record", Source::Builtin, Kind::Keyword);
        has("total = 1\ntot", "a.py", "total", Source::Word, Kind::Word);
        // Each language of the table has names of its own.
        for (file, typed, label) in [
            ("a.go", "app", "append"),
            ("a.java", "StringB", "StringBuilder"),
            ("a.c", "mal", "malloc"),
            ("a.cpp", "uniq", "unique_ptr"),
            ("a.cs", "Cons", "Console"),
            ("a.php", "array_m", "array_map"),
            ("a.rb", "attr_a", "attr_accessor"),
            ("a.kt", "listO", "listOf"),
            ("a.swift", "fatal", "fatalError"),
            ("a.sql", "COAL", "COALESCE"),
            ("a.sh", "ech", "echo"),
        ] {
            has(typed, file, label, Source::Builtin, Kind::Keyword);
        }
        // A macro is written with its `!`, and not a second time as a word of the file.
        let found = suggested("println!(\"a\");\npri", "a.rs");
        assert_eq!(found[place(&found, "println!")].body, "println!");
        assert!(from(&found, Source::Word).is_empty());
    }

    #[test]
    fn the_sources_are_ranked_together() {
        let found = suggested("let matcher = 1;\nmat", "a.rs");
        let first: Vec<(&str, Source)> = found
            .iter()
            .map(|found| (found.label.as_str(), found.source))
            .take(6)
            .collect();
        // As good a match: snippets, keywords, names of the language, words.
        assert_eq!(
            first,
            [
                ("match", Source::Snippet),
                ("matchoption", Source::Snippet),
                ("matchresult", Source::Snippet),
                ("match", Source::Keyword),
                ("matches!", Source::Builtin),
                ("matcher", Source::Word),
            ]
        );
        // What only has the letters comes after all that starts with them.
        assert!(place(&found, "format!") > place(&found, "matcher"));
        // A better match comes first, whatever its source: the case typed.
        let found = suggested("let result = 1;\nres", "a.rs");
        assert!(place(&found, "result") < place(&found, "Result"));
        let found = suggested("let result = 1;\nRes", "a.rs");
        assert!(place(&found, "Result") < place(&found, "result"));
        // The snippet typed whole is the first: Tab writes it.
        for (file, opening) in [("a.rs", "fn main() {"), ("a.py", "def main():")] {
            let found = suggested("mainly = 1\nmain", file);
            assert_eq!(
                (found[0].label.as_str(), found[0].source),
                ("main", Source::Snippet)
            );
            assert!(found[0].body.contains(opening), "{file}");
            assert!(place(&found, "mainly") > 0);
        }
        assert!(found.len() <= MAX_COMPLETIONS);
    }

    #[test]
    fn the_snippets_of_the_user_are_suggested_with_the_others() {
        let snippets = [
            ("main".to_owned(), "mine($1)".to_owned()),
            ("zz log".to_owned(), "console.log($1)".to_owned()),
        ];
        let language = Language::of(Path::new("a.js"));
        let ask = |text: &'static str| {
            suggest(&Request {
                snippets: &snippets,
                ..Request::new(text, text.chars().count(), language)
            })
        };
        // One of the user takes the place of the one that comes under its name.
        let found = ask("main");
        assert_eq!(found[0].body, "mine($1)");
        assert_eq!(from(&found, Source::Snippet), ["main"]);
        let found = ask("zl");
        assert_eq!(
            (found[0].label.as_str(), &found[0].matched),
            ("zz log", &vec![0, 3])
        );
    }

    #[test]
    fn the_words_nearer_the_caret_come_first() {
        let text = "count_far = 1\n\n\n\ncount_nearer = 2\ncou";
        let found = suggested(text, "a.py");
        assert_eq!(from(&found, Source::Word), ["count_nearer", "count_far"]);
        // Below the caret as well as above it.
        let text = "cou\ncount_nearer = 2\n\n\n\ncount_far = 1";
        let language = Language::of(Path::new("a.py"));
        let found = suggest(&Request::new(text, 3, language));
        assert_eq!(from(&found, Source::Word), ["count_nearer", "count_far"]);
        // A better match is still before a nearer one.
        let found = suggested("count = 1\n\n\n\nrecount_all = 2\ncou", "a.py");
        assert_eq!(from(&found, Source::Word), ["count", "recount_all"]);
    }

    #[test]
    fn the_word_being_typed_is_not_suggested() {
        let found = suggested("counter = 1\ncoun", "a.py");
        assert_eq!(from(&found, Source::Word), ["counter"]);
        // Nor is it when the caret is in the middle of it.
        let text = "value_one = 1\nval_x";
        let found = suggest(&Request::new(text, 17, Language::of(Path::new("a.py"))));
        assert_eq!(from(&found, Source::Word), ["value_one"]);
        // A word typed in full has nothing left to complete; a number is no word.
        let found = suggested("total = 1\ntotal", "a.py");
        assert!(found.iter().all(|found| found.label != "total"));
        assert!(suggested("total = 1\n", "a.py").is_empty());
        assert!(suggested("total = 12", "a.py").is_empty());
        // Words beyond ASCII are words, what is between them is not.
        let found = suggested("étage — étape = 1\nét", "a.py");
        assert_eq!(from(&found, Source::Word), ["étage", "étape"]);
    }

    #[test]
    fn the_words_of_the_other_files_come_after_those_of_this_one() {
        let language = Language::of(Path::new("a.py"));
        let others = [
            "tot_a = 1\ntotal_there = 2",
            "total_here = 3\ntotal_far = 4",
        ];
        let text = "total_here = 1\ntot";
        let found = suggest(&Request {
            others: &others,
            ..Request::new(text, text.chars().count(), language)
        });
        assert_eq!(from(&found, Source::Word), ["total_here"]);
        // Each once, the shorter first: none is nearer than another.
        assert_eq!(
            from(&found, Source::Other),
            ["tot_a", "total_far", "total_there"]
        );
        assert!(place(&found, "total_here") < place(&found, "tot_a"));
        // Alone, they are suggested all the same.
        let found = suggest(&Request {
            others: &others,
            ..Request::new("tot", 3, language)
        });
        assert_eq!(found[0].label, "tot_a");
    }

    #[test]
    fn after_a_dot_the_members_used_in_the_files_come_first() {
        let text = "const box = view.getTextBounds();\nview.width = 2;\nconst getter = 1;\nbox.ge";
        let found = suggested(text, "a.js");
        assert_eq!(
            (found[0].label.as_str(), found[0].source),
            ("getTextBounds", Source::Member)
        );
        assert_eq!(found[0].matched, [0, 1]);
        // The other words are still there, after them; no snippet, no name of the language.
        assert_eq!(from(&found, Source::Word), ["getter"]);
        assert!(from(&found, Source::Snippet).is_empty());
        assert!(from(&found, Source::Builtin).is_empty());
        // Nothing typed yet: the members alone, the nearer first.
        let found = suggested("a.width = 1;\nb.height = 2;\nconst other = 3;\na.", "a.js");
        let labels: Vec<&str> = found.iter().map(|found| found.label.as_str()).collect();
        assert_eq!(labels, ["height", "width"]);
        assert_eq!(found[0].replace, 46..46);
        // Those of the other files after those of this one; short ones are members too.
        let language = Language::of(Path::new("a.js"));
        let text = "a.width = 1;\na.";
        let found = suggest(&Request {
            others: &["b.id = 2; b.width = 3;"],
            ..Request::new(text, text.chars().count(), language)
        });
        assert_eq!(from(&found, Source::Member), ["width", "id"]);
        // Two dots are a range, not a member.
        let found = suggested("let length = 3;\nfoo.lead();\nfor i in 0..le", "a.rs");
        assert!(from(&found, Source::Member).is_empty());
        assert_eq!(from(&found, Source::Word), ["lead", "length"]);
    }

    #[test]
    fn after_two_colons_the_names_used_after_them_come_first() {
        let text = "use std::collections::HashMap;\nlet a = Vec::new();\nlet b = String::";
        let found = suggested(text, "a.rs");
        let labels: Vec<&str> = found.iter().map(|found| found.label.as_str()).collect();
        assert_eq!(labels, ["new", "HashMap", "collections"]);
        let found = suggested(&format!("{text}ne"), "a.rs");
        assert_eq!(
            (found[0].label.as_str(), found[0].source),
            ("new", Source::Member)
        );
    }

    #[test]
    fn a_path_in_a_string_is_completed_one_step_at_a_time() {
        let files = [
            "utils.js",
            "lib/math.js",
            "./lib/text.js",
            "../shared/api.js",
        ];
        let language = Language::of(Path::new("a.js"));
        let ask = |text: &str| {
            suggest(&Request {
                files: &files,
                ..Request::new(text, text.chars().count(), language)
            })
        };
        let labels = |text: &str| {
            let found = ask(text);
            assert!(found.iter().all(|found| found.source == Source::Path));
            found
                .into_iter()
                .map(|found| found.label)
                .collect::<Vec<_>>()
        };
        // What is in the folder: each folder once, with its `/`.
        assert_eq!(labels("import x from \"./"), ["../", "lib/", "utils.js"]);
        assert_eq!(labels("import x from \""), ["../", "lib/", "utils.js"]);
        assert_eq!(labels("const x = require('./ut"), ["utils.js"]);
        assert_eq!(labels("import(`./lib/"), ["math.js", "text.js"]);
        assert_eq!(labels("import x from \"../"), ["shared/"]);
        assert_eq!(labels("import x from \"../shared/"), ["api.js"]);
        // Only the step being typed is written over.
        let mut text = "import x from \"./li".to_owned();
        let found = ask(&text);
        assert_eq!(
            (found[0].label.as_str(), &found[0].matched),
            ("lib/", &vec![0, 1])
        );
        let written = accept(&mut text, &found[0], "    ");
        assert_eq!(text, "import x from \"./lib/");
        assert_eq!(written.first(), 21..21);
        let found = ask(&format!("{text}mt"));
        assert_eq!(found[0].label, "math.js");
        text.push_str("mt");
        accept(&mut text, &found[0], "    ");
        assert_eq!(text, "import x from \"./lib/math.js");
        // A string that is no path, a string that is closed, no file given: words as ever.
        for text in ["const s = \"hello li", "import x from \"./lib\"; li"] {
            assert!(ask(text).iter().all(|found| found.source != Source::Path));
        }
        assert!(suggested("import x from \"./", "a.js").is_empty());
        // A name is written as it is, whatever is in it.
        let found = suggest(&Request {
            files: &["$types}.ts"],
            ..Request::new("import x from \"./", 17, language)
        });
        let mut text = "import x from \"./".to_owned();
        accept(&mut text, &found[0], "    ");
        assert_eq!(text, "import x from \"./$types}.ts");
    }

    #[test]
    fn paths_are_suggested_where_each_language_takes_one() {
        let ask = |text: &str, file: &str, files: &[&str]| {
            let found = suggest(&Request {
                files,
                ..Request::new(text, text.chars().count(), Language::of(Path::new(file)))
            });
            let paths = found
                .into_iter()
                .filter(|found| found.source == Source::Path);
            paths.map(|found| found.label).collect::<Vec<_>>()
        };
        assert_eq!(
            ask("#include \"ut", "a.c", &["util.h", "main.c"]),
            ["util.h"]
        );
        let page = ["images/a.png", "images/b.png", "style.css"];
        assert_eq!(ask("<img src=\"im", "a.html", &page), ["images/"]);
        assert_eq!(ask("<link href=\"st", "a.html", &page), ["style.css"]);
        assert_eq!(
            ask("a { background: url(\"./", "a.css", &page),
            ["images/", "style.css"]
        );
        // A link to another site is no path.
        assert!(ask("<a href=\"https://st", "a.html", &page).is_empty());
        // A module of Rust: a file beside this one, or a folder with its `mod.rs`.
        let crate_files = [
            "parser.rs",
            "lib.rs",
            "syntax/mod.rs",
            "main.rs",
            "data/a.rs",
        ];
        assert_eq!(ask("mod pa", "lib.rs", &crate_files), ["parser"]);
        assert_eq!(
            ask("pub mod ", "lib.rs", &crate_files),
            ["parser", "syntax"]
        );
        assert!(ask("let model = pa", "lib.rs", &crate_files).is_empty());
        assert!(ask("mod pa", "a.py", &crate_files).is_empty());
    }

    #[test]
    fn markup_and_style_sheets_are_suggested_what_they_are_completed_with() {
        for (text, file) in [
            ("<a h", "a.html"),
            ("<input type=\"pa", "a.html"),
            ("<p style=\"color: red; disp", "a.html"),
            ("a {\n  font-s", "a.css"),
            ("a {\n  display: fl", "a.css"),
        ] {
            let language = Language::of(Path::new(file));
            let known = complete(text, text.chars().count(), language);
            let found = suggested(text, file);
            let same: Vec<Completion> = found.iter().map(Suggestion::completion).collect();
            assert!(!known.is_empty() && same == known, "{text}");
        }
        // What is typed of a name with hyphens is all of it; the word is written over.
        let mut text = "a {\n  font-s".to_owned();
        let found = suggested(&text, "a.css");
        assert_eq!(found[0].label, "font-size");
        assert_eq!(found[0].matched, [0, 1, 2, 3, 4, 5]);
        let written = accept(&mut text, &found[0], "    ");
        assert_eq!(text, "a {\n  font-size: ;");
        assert_eq!(written.first(), 17..17);
        // Elsewhere in a page: the words, as in any file.
        let found = suggested("<p>paragraph</p>\n<p>par", "a.html");
        assert_eq!(from(&found, Source::Word), ["paragraph"]);
    }

    #[test]
    fn every_snippet_that_comes_with_a_language_is_well_formed() {
        for (language, snippets) in STOPS_SNIPPETS {
            for (label, body) in *snippets {
                let made = expand(body, "    ", "");
                let name = format!("{language} {label}: {}", made.text);
                // A pattern of a `case` of the shell ends with a `)` that opens nothing.
                let text = match *language {
                    "Shell" => made.text.replace("*)", "*"),
                    _ => made.text.clone(),
                };
                let mut open = Vec::new();
                for ch in text.chars() {
                    match ch {
                        '(' | '[' | '{' => open.push(ch),
                        ')' => assert_eq!(open.pop(), Some('('), "{name}"),
                        ']' => assert_eq!(open.pop(), Some('['), "{name}"),
                        '}' => assert_eq!(open.pop(), Some('{'), "{name}"),
                        _ => {}
                    }
                }
                assert!(open.is_empty(), "{name}");
                // Nothing of a tab stop is left: a `$` is one of the language (PHP).
                let stop =
                    |after: &str| after.starts_with(|ch: char| ch.is_ascii_digit() || ch == '{');
                let left = made.text.match_indices('$');
                assert!(
                    !left.into_iter().any(|(at, _)| stop(&made.text[at + 1..])),
                    "{name}"
                );
                assert_eq!(
                    made.text.contains('$'),
                    *language == "PHP" && body.contains("\\$")
                );
                // Its stops are numbered from 1 with none missing, and are in the text.
                let numbers: Vec<u32> = made.stops.iter().map(|stop| stop.number).collect();
                assert!(
                    numbers.iter().copied().eq(1..=numbers.len() as u32),
                    "{name}"
                );
                let length = made.text.chars().count();
                let places = made.stops.iter().flat_map(|stop| &stop.ranges);
                assert!(
                    places.chain([&made.end]).all(|place| place.end <= length),
                    "{name}"
                );
            }
            let labels: HashSet<&str> = stops_snippets_of(language)
                .map(|(label, _)| label)
                .collect();
            assert_eq!(
                labels.len(),
                stops_snippets_of(language).count(),
                "{language}"
            );
        }
        // None of those `complete` offers is missing.
        for (language, snippets) in SNIPPETS {
            for (label, _) in *snippets {
                let found = stops_snippets_of(language).any(|(found, _)| found == *label);
                assert!(found, "{language} {label}");
            }
        }
    }

    /// The stops of `made`, as `1: 4..8 12..16, 2: 20..20`.
    fn stops(made: &Expanded) -> String {
        let places = |stop: &Stop| {
            let places = stop.ranges.iter().map(|place| format!("{place:?}"));
            format!("{}: {}", stop.number, places.collect::<Vec<_>>().join(" "))
        };
        let stops: Vec<String> = made.stops.iter().map(places).collect();
        stops.join(", ")
    }

    #[test]
    fn a_snippet_is_expanded_into_its_text_and_its_tab_stops() {
        // Plain stops: nothing is written for them.
        let made = expand("fn $1($2) {\n\t$0\n}", "  ", "");
        assert_eq!(made.text, "fn () {\n  \n}");
        assert_eq!(stops(&made), "1: 3..3, 2: 4..4");
        assert_eq!((made.end.clone(), made.first()), (10..10, 3..3));
        // Placeholders: their text is written, and is the stop.
        let made = expand("let ${1:name} = ${2:value};", "  ", "");
        assert_eq!(made.text, "let name = value;");
        assert_eq!(stops(&made), "1: 4..8, 2: 11..16");
        assert_eq!(made.end, 17..17);
        // Whatever the order they are written in, they are walked by number.
        let made = expand("${2:b} ${1:a} ${3}", "  ", "");
        assert_eq!(stops(&made), "1: 2..3, 2: 0..1, 3: 4..4");
        // No stop at all: the end is the place.
        let made = expand("done", "  ", "");
        assert_eq!(stops(&made), "");
        assert_eq!((made.end.clone(), made.first()), (4..4, 4..4));
    }

    #[test]
    fn a_stop_written_twice_holds_the_same_text_in_each_place() {
        let made = expand("for (let ${1:i} = 0; ${1:i} < ${2:n}; $1++) {}", "  ", "");
        assert_eq!(made.text, "for (let i = 0; i < n; i++) {}");
        assert_eq!(stops(&made), "1: 9..10 16..17 23..24, 2: 20..21");
        // The text is the one it is first given, wherever that is.
        let made = expand("$1 = ${1:x}", "  ", "");
        assert_eq!(made.text, "x = x");
        assert_eq!(stops(&made), "1: 0..1 4..5");
        // A stop that holds itself does not go round and round.
        assert_eq!(expand("${1:a$1}", "  ", "").text, "aa");
    }

    #[test]
    fn escapes_choices_nesting_and_variables_are_read_as_vs_code_does() {
        // `\$`, `\}` and `\\` are the chars; any other backslash stays.
        let made = expand("cost: \\$5 \\} \\\\ $1 a\\nb", "  ", "");
        assert_eq!(made.text, "cost: $5 } \\  a\\nb");
        assert_eq!(stops(&made), "1: 13..13");
        // A `$` that starts no stop is a `$`.
        for body in ["echo $item;", "$", "a $ b", "${item}", "${", "$_x"] {
            assert_eq!(expand(body, "  ", "").text, body);
        }
        // `$1abc` is the stop 1 and `abc`.
        let made = expand("$1abc", "  ", "");
        assert_eq!(
            (made.text.as_str(), stops(&made).as_str()),
            ("abc", "1: 0..0")
        );
        // A placeholder in a placeholder: both are stops of the one text.
        let made = expand("${1:foo ${2:bar}}!", "  ", "");
        assert_eq!(made.text, "foo bar!");
        assert_eq!(stops(&made), "1: 0..7, 2: 4..7");
        assert_eq!(expand("${1:a \\} b}", "  ", "").text, "a } b");
        // One never closed, or nested without end, is still text.
        assert_eq!(expand("${1:open", "  ", "").text, "open");
        let deep = "${1:".repeat(200);
        assert!(expand(&deep, "  ", "").text.len() <= deep.len());
        // Of the choices, the first.
        let made = expand("${1|one,two,three|} $0", "  ", "");
        assert_eq!(made.text, "one ");
        assert_eq!((stops(&made).as_str(), made.end.clone()), ("1: 0..3", 4..4));
        assert_eq!(expand("${1|a\\,b,c|}", "  ", "").text, "a,b");
        // What VS Code knows by name is nothing here, or its default.
        let made = expand(
            "$TM_FILENAME|${TM_SELECTED_TEXT:none}|${CLIPBOARD}|",
            "  ",
            "",
        );
        assert_eq!(made.text, "|none||");
        // What rewrites a text is left out: the stop stays one, with its text.
        let made = expand(
            "${1:name} ${1/(.*)/${1:/upcase}/} ${TM_FILENAME/x{2}/y/}.",
            "  ",
            "",
        );
        assert_eq!(made.text, "name name .");
        assert_eq!(stops(&made), "1: 0..4 5..9");
    }

    #[test]
    fn the_last_place_is_where_the_zero_is() {
        let made = expand("$0after ${1:first}", "  ", "");
        assert_eq!(made.text, "after first");
        assert_eq!((made.end.clone(), made.first()), (0..0, 6..11));
        // It can hold a text, to type over.
        let made = expand("def ${1:f}():\n\t${0:pass}", "    ", "");
        assert_eq!(made.text, "def f():\n    pass");
        assert_eq!((stops(&made).as_str(), made.end), ("1: 4..5", 13..17));
    }

    #[test]
    fn the_lines_of_a_snippet_are_indented_and_its_stops_follow() {
        // A tab is one unit, each line after the first starts as the line written on.
        let made = expand("if ${1:ok} {\n\t$0\n}", "\t", "    ");
        assert_eq!(made.text, "if ok {\n    \t\n    }");
        assert_eq!((stops(&made).as_str(), made.end), ("1: 3..5", 13..13));
        // Lines that end with a carriage return keep it, and it counts as a char.
        let made = expand("a$1\r\n\tb$0\r\nc", "  ", "    ");
        assert_eq!(made.text, "a\r\n      b\r\n    c");
        assert_eq!((stops(&made).as_str(), made.end), ("1: 1..1", 10..10));
        // Written in a text: the stops are where they are in it.
        let mut text = "x = 1\n    for".to_owned();
        let written = write_stops(
            &mut text,
            10..13,
            "for ${1:i} in ${2:all}:\n\t${0:pass}",
            "  ",
        );
        assert_eq!(text, "x = 1\n    for i in all:\n      pass");
        assert_eq!(stops(&written), "1: 14..15, 2: 19..22");
        assert_eq!((written.end.clone(), written.first()), (30..34, 14..15));
        assert_eq!(written.text, "for i in all:\n      pass");
    }

    #[test]
    fn a_snippet_with_tab_stops_is_applied_with_nothing_of_them_left() {
        // As a completion: the caret goes to the first stop.
        let mut text = "class A:\n    de".to_owned();
        let picked = completion("def", "def ${1:name}($2):\n\t${0:pass}", Kind::Snippet);
        let caret = apply(&mut text, 15, &picked, "    ");
        assert_eq!(text, "class A:\n    def name():\n        pass");
        assert_eq!(caret, 17);
        // With no stop but `$0`, or none at all: as ever.
        let mut text = "ab".to_owned();
        assert_eq!(write_snippet(&mut text, 0..2, "a($0)b", "  "), 2);
        assert_eq!(write_snippet(&mut text, 4..4, "!", "  "), 5);
        assert_eq!(text, "a()b!");
        // As a suggestion: the stops come with it.
        let mut text = "class A:\n    de".to_owned();
        let found = suggested(&text, "a.py");
        assert_eq!(
            (found[0].label.as_str(), found[0].source),
            ("def", Source::Snippet)
        );
        let written = accept(&mut text, &found[0], "    ");
        assert_eq!(text, "class A:\n    def name():\n        pass");
        assert_eq!(stops(&written), "1: 17..21, 2: 22..22");
        assert_eq!(written.end, 33..37);
        // Every snippet of every language, written whole by its name.
        for (language, snippets) in STOPS_SNIPPETS {
            for (label, _) in *snippets {
                let language = Language::named(language).expect(language);
                let found = suggest(&Request::new(label, label.chars().count(), language));
                assert_eq!(found[0].label, *label, "{}", language.name);
                let mut text = (*label).to_owned();
                let written = accept(&mut text, &found[0], "    ");
                assert_eq!(text, written.text, "{label}");
            }
        }
    }

    #[test]
    fn what_is_typed_in_a_stop_is_written_in_its_other_places() {
        let body = "for (let ${1:i} = 0; ${1:i} < ${2:n}; ${1:i}++) {\n\t$0\n}";
        let mut text = String::new();
        let mut written = write_stops(&mut text, 0..0, body, "  ");
        assert_eq!(written.end, 32..32);
        // `idx` typed over the `i` selected.
        text.replace_range(9..10, "idx");
        assert!(written.follow(&mut text, 0, 9, 1, 3));
        assert_eq!(text, "for (let idx = 0; idx < n; idx++) {\n  \n}");
        assert_eq!(stops(&written), "1: 9..12 18..21 27..30, 2: 24..25");
        assert_eq!(written.end, 38..38);
        // One char more, then one erased in the middle.
        text.insert(12, '2');
        assert!(written.follow(&mut text, 0, 12, 0, 1));
        text.remove(10);
        assert!(written.follow(&mut text, 0, 10, 1, 0));
        assert_eq!(text, "for (let ix2 = 0; ix2 < n; ix2++) {\n  \n}");
        assert_eq!(stops(&written), "1: 9..12 18..21 27..30, 2: 24..25");
        // The next stop, with chars beyond ASCII.
        text.replace_range(24..25, "été");
        assert!(written.follow(&mut text, 1, 24, 1, 3));
        assert_eq!(text, "for (let ix2 = 0; ix2 < été; ix2++) {\n  \n}");
        assert_eq!(stops(&written), "1: 9..12 18..21 29..32, 2: 24..27");
        assert_eq!(written.end, 40..40);
        // Typed anywhere else: the snippet is left.
        assert!(!written.follow(&mut text, 0, 0, 0, 1));
        assert!(!written.follow(&mut text, 5, 9, 0, 1));
        // Stops at the same place: what is typed in the first pushes the others.
        let mut text = String::new();
        let mut written = write_stops(&mut text, 0..0, "$1$2", "  ");
        text.push('a');
        assert!(written.follow(&mut text, 0, 0, 0, 1));
        assert_eq!(stops(&written), "1: 0..1, 2: 1..1");
        assert_eq!(written.end, 1..1);
    }

    /// A file of about `bytes` bytes of code: thousands of names, used over and over.
    fn big_file(bytes: usize) -> String {
        let parts = [
            "get", "set", "text", "bounds", "line", "count", "view", "item", "node", "path",
            "open", "close", "read", "write", "map", "key", "value", "index", "size", "name",
            "buffer", "token", "range", "state", "width",
        ];
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let name = |next: &mut dyn FnMut(usize) -> usize| {
            let camel = next(2) == 0;
            let mut name = String::new();
            for part in 0..2 + next(3) {
                let word = parts[next(parts.len())];
                match (camel, part) {
                    (_, 0) => name.push_str(word),
                    (true, _) => {
                        name.push_str(&word[..1].to_uppercase());
                        name.push_str(&word[1..]);
                    }
                    (false, _) => {
                        name.push('_');
                        name.push_str(word);
                    }
                }
            }
            name
        };
        let mut text = String::with_capacity(bytes + 128);
        while text.len() < bytes {
            let names: Vec<String> = (0..4).map(|_| name(&mut next)).collect();
            let [a, b, c, d] = names.as_slice() else {
                unreachable!()
            };
            text.push_str(&format!("    let {a} = {b}.{c}({d}); // {a}\n"));
        }
        text
    }

    #[test]
    #[ignore = "a measure: cargo test --release -p godcode-core a_big_file -- --ignored --nocapture"]
    fn a_big_file_is_completed_in_a_few_milliseconds() {
        let language = Language::of(Path::new("a.rs"));
        let file = big_file(2 * 1024 * 1024);
        let other = big_file(256 * 1024);
        let names: HashSet<&str> = file.split(|ch: char| !is_word_char(ch)).collect();
        println!(
            "{} bytes, {} lines, {} names",
            file.len(),
            file.lines().count(),
            names.len()
        );
        for (typed, others) in [
            ("gtb", 0),
            ("g", 0),
            ("ge", 0),
            ("getText", 0),
            ("view.", 0),
            ("view.gtb", 0),
            ("gtb", 4),
        ] {
            let text = format!("{file}    {typed}");
            let others = vec![other.as_str(); others];
            let request = Request {
                others: &others,
                ..Request::new(&text, text.chars().count(), language)
            };
            let mut times: Vec<std::time::Duration> = (0..21)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let found = suggest(&request);
                    assert!(!found.is_empty(), "{typed}");
                    started.elapsed()
                })
                .collect();
            times.sort();
            let known = std::time::Instant::now();
            let completed = complete(&text, text.chars().count(), language).len();
            let known = known.elapsed();
            println!(
                "{typed:>9} + {} other files: median {:?}, best {:?}, worst {:?} (complete: {known:?} for {completed})",
                others.len(),
                times[10],
                times[0],
                times[20],
            );
            // Not in a debug build: it is some twenty times slower.
            if !cfg!(debug_assertions) {
                assert!(times[10].as_millis() < 50, "{typed}: {:?}", times[10]);
            }
        }
    }
}
