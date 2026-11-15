# Edit an existing Contract app

Use the provided app root; find `app.contract` and its configuration if unknown.
Read the parts needed for the requested change and preserve other behavior.
Reuse project context, identifiers and bindings already established in the
conversation. Look up documentation when a needed form or name is unknown.
Generated plans and type declarations are compiler outputs.

## Read the language

Contract uses indentation for component, action and view bodies. Components can
declare `props`, `state`, `derive` and actions; the root also owns resources and
mutations. A component's `view` contains elements, component calls and conditional
or repeated regions. Existing
bindings and module interfaces are authoritative; preserve their names and types.

Use `and`, `or` and `not` for boolean expressions, `==` for equality and
`condition ? first : second` for a conditional value. Parentheses group attribute
expressions. `pending(resourceOrMutation)` reads pending status. Options use
`none` and `some(value)`. Strings use double quotes; templates use backticks and `${value}`.

Common view tags include `column`, `row`, `text`, `button`, `input`, `textarea` and
`link`. Text can be a child, such as `text "Save"`, or a button's positional label.
Attributes mix CSS and platform names: `aria-label`, `font-size`, `testId` and
`retainFocus` retain their exact spelling. Source identifiers are case-sensitive.
Consult the vocabulary instead of assuming an HTML tag or property name.

An input's `value`, `input`, `change` and `submit` bindings have different roles.
Preserve bindings, disabled conditions and focus overrides unless the task asks
to change them. Actions read starting state; their assignments commit together.

## Locate the authored control

Use its component, visible label, attribute or binding to find the source.
Use exact identifiers or visible text supplied by the request or established in
earlier inspection. A description such as "server name input" does not establish
a `testId`; discover the actual source anchor when it is unknown.
Identical labels can occur in menus, platform branches or component instances.
Keep necessary searches narrow and batch independent lookups. Read current values
when the edit needs them; preserving unrelated properties does not require reading
them again. Do not repeat file discovery or reread earlier accepted diffs between
requests without a new reason.

The app's `bun exact.mjs contract` command invokes its Exact compiler. Useful
lookups are `symbols app.contract --name NAME` for a declaration/interface and
`vocab NAME` for a tag, attribute, function or recipe. Ordinary source search is
also available. Read only the needed section of the longer guides.

For example, `aria-label="Save"` is a string value, while
`disabled=pending(saved)` is an expression. Use an existing resource or mutation
name inside `pending`; discover the declaration when the request does not supply it.

## Apply and check the change

Batch independent edits when their targets and values are known. Use ordinary
text editing for statements and unsupported structural operations. Keep unrelated
authored source intact.

Validate the final source revision with
`bun exact.mjs contract build app.contract -o check.plan --json --errors-only`.
Review the resulting diff. A structural edit's successful applied receipt already
compiles its exact Contract revision and contains that diff; review the receipt
without repeating that compiler check. The app wrapper's build can also refresh
generated TypeScript types and check `app.ts`; run those checks when affected.
Later source modifications need fresh validation.
Compilation checks the program; exercise changed behavior on the actual app when
that app and host are available.

The grammar reference describes exact forms. The agent guide covers state/action
semantics, data requests, views and structural source editing. The compiler and
executable fixtures establish supported behavior.
