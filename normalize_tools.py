"""Idempotent field normalizer for descriptors.rs.

Every ConnectorTool must declare danger / b64_params / raw_params / base_override.
I kept adding a field, THEN inserting new tools, so the new tools missed the
seeding pass — three build failures from the same ordering mistake. This runs
over every block and adds only what's missing, so order stops mattering and it's
safe to re-run after adding tools.
"""
import re, sys

p = 'src-tauri/src/connectors/descriptors.rs'
s = open(p).read()

DEFAULTS = [
    ('danger', 'danger: false,'),
    ('b64_params', 'b64_params: &[],'),
    ('raw_params', 'raw_params: &[],'),
    ('base_override', 'base_override: "",'),
]

parts = re.split(r'(ConnectorTool \{)', s)
out = [parts[0]]
added = 0
for i in range(1, len(parts), 2):
    head, body = parts[i], parts[i + 1]
    # Only inspect this tool's own field region (before `render:`), so a nested
    # brace in a body template can't confuse the check.
    region = body.split('render:')[0]
    m = re.match(r'\n(\s*)', body)
    ind = m.group(1) if m else '            '
    prefix = ''
    for field, decl in DEFAULTS:
        if not re.search(r'\b%s:' % field, region):
            prefix += '\n' + ind + decl
            added += 1
    out.append(head)
    out.append(prefix + body if prefix else body)

s = ''.join(out)
open(p, 'w').write(s)
print(f"normalized: {added} missing field(s) added")
