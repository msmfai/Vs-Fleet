# Eval Artifacts

This directory is the default local output location for `make eval`.

The generated JSON/XML/HTML reports and screenshots are ignored in git because
they can contain local paths, workspace contents, old failing rows, screenshots,
and machine-state details. Publish them as CI artifacts or attach a curated,
redacted subset to an issue or release when needed.
