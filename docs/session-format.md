# Session format

The entire harness state is one append-only file: `.masse/session.csv`, kept in
the workspace's `.masse/` directory.

There is no `history/` directory and no `state.json`. The state is reconstructed
by reading the CSV; file history is Git's job. masse ensures `.masse/` and
`_db.sqlite*` are listed in the workspace `.gitignore`.

## Columns

```
timestamp,event_type,role,tool,args,status,output,exit_code
```

| Column | Meaning |
| --- | --- |
| `timestamp` | RFC 3339 UTC with milliseconds, e.g. `2026-10-08T12:00:00.000Z` |
| `event_type` | `session_start`, `user_message`, `assistant_message`, `tool_call`, `tool_result`, `session_end` |
| `role` | `system`, `user`, `assistant`, `tool` (empty when not applicable) |
| `tool` | tool name for `tool_call` / `tool_result` |
| `args` | encoded arguments map |
| `status` | `ok` or `error` (tool results only) |
| `output` | free text: message body or tool output |
| `exit_code` | process exit code for CLI tools, empty otherwise |

Columns that do not apply to an event are left empty.

## Encoding: one event per line

The goal is that **every record is exactly one physical line**.

- Backslash, LF and CR are escaped in every field: `\` → `\\`, newline → `\n`,
  carriage return → `\r`. A tool output that contained newlines therefore stays
  on one line, and `output` reads back as the original text after unescaping.
- After escaping, RFC 4180 quoting is applied only when a comma or a double
  quote remains; internal double quotes are doubled (`"` → `""`).
- The reader is a proper CSV state machine (`harness_session::csv`) and also
  tolerates legacy records that used real newlines inside quoted fields.

Example (a single line):

```
2026-10-08T12:00:07.010Z,tool_result,tool,read_file,path=main.wren,ok,"fn main() {\n  System.print(\"hi\")\n}",0
```

## `args` encoding

`args` is a `k=v` map joined with `|`. Values are percent-encoded so they can
contain `|`, `=` and newlines:

```
path=src%2Fa%2Cb.wren|pattern=foo%7Cbar%3D1
```

Encoding rules: `A–Z a–z 0–9 - _ . ~` are left as-is; every other byte becomes
`%XX` (uppercase hex). `harness_session::encode_json_args` converts a JSON
argument object into this form; `decode_args` reverses it.

## Rehydration rules

Reading the file back produces the conversation:

| Event | Reconstructed as |
| --- | --- |
| `user_message` | user message |
| `assistant_message` | assistant message (skipped when empty) |
| `tool_call` / `tool_result` | user-role note `[tool <name> → <status>]\n<output>` |

The CSV does not store an OpenAI `tool_call_id`, so replayed tool results cannot
be tool-role messages. They are replayed as user-role notes instead. Live turns
keep full fidelity because the in-memory history holds the real ids.
