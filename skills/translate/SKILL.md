---
name: translate
description: Translate changed blog posts into another language, preserving MDX, frontmatter and code blocks. Use it when a post needs a translated counterpart.
---

# Translate

You translate blog posts into the language given by the `lang` argument (a
language code such as `en`). Translations are placed next to the original
following the `naming` convention from the prompt:

- `{dir}` is the original's directory, `{ext}` the extension, `{lang}` the target
  language;
- `{stem}` is the file name without extension and **without a language code**: if
  the original name already carries one (`index.ru.md`), it is replaced rather
  than appended. So `content/posts/dns/index.ru.md` + `lang=en` becomes
  `content/posts/dns/index.en.md`, and `posts/hello.mdx` becomes
  `posts/hello.en.mdx`;
- if the translated file already exists, overwrite it completely.

## What to do

1. Read the original of every file in the list of changed files.
2. Create (or overwrite) the translation file following the naming convention.
3. Translate the prose. Keep the following unchanged:
   - the MDX structure and the JSX components, their names and attributes (only
     the text visible to a reader gets translated);
   - imports and code blocks — code itself is never translated; comments inside
     code are translated only when they are in the source language;
   - links, image paths, anchors;
   - markup: headings, lists, tables, quotes, footnotes.
4. In the frontmatter, translate only the human-facing fields (`title`,
   `description`, `summary`, and `tags` when they are meaningful words). Leave
   `slug`, `date`, `draft`, `layout`, identifiers and any technical field alone.
5. Preserve the tone of the original: informal, plain, free of bureaucratese.

## What not to do

- Do not edit the original — it stays as it is.
- Do not translate files that are not in the list of changed files.
- Do not create any file other than the translations.
- Do not commit and do not touch git: the orchestrator handles that.

## When you are done

Write a short summary to `/out/summary.md`: which files you translated, into
which language, and what you deliberately left untouched (terms that stay in the
original language, for instance).
