---
title: "Quoted: with a colon"
aliases:
  - Alt Name
  - "Another: alias"
tags: [a, b, "c d"]
nested:
  key: value
  list:
    - one
    - two
number: 42
float: 3.14
bool: true
empty:
date: 2026-07-28
multiline: |
  line one
  line two
    indented
---

# Messy Frontmatter

Body starts here. Do **not** touch the YAML above — key order, quoting, the
literal block scalar, and the nested list must all survive byte-for-byte.

- a list item with a #tag inside
- a [[wikilink]] and a [[link|alias]] and a [[link#heading]] and a [[link^blk]]
- an ![[embed.png]]

> [!note] A callout
> with a second line

```yaml
# this fenced block LOOKS like frontmatter but is code — never parse it
key: not-real
```

Trailing paragraph with no newline weirdness.
