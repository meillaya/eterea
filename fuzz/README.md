# Fuzz targets

Standalone cargo-fuzz crate. It is not a member of the root workspace.

```bash
bash scripts/verify/fuzz.sh                 # 10 s per target
FUZZ_TIME=30 bash scripts/verify/fuzz.sh    # longer local run
bash scripts/verify/fuzz.sh url_policy      # one target
```

The script keeps the working corpus under `.tools/fuzz-corpus/`, artifacts
under `fuzz/artifacts/`, and the compiler temp dir under `.tools/tmp/`, so a run
does not write under the user's home. Only the seeds in `fuzz/corpus/` are
committed.

Targets:

- `url_policy`: arbitrary UTF-8 into `is_safe_https_url`/`is_image_mime`.
  Accepted URLs must be HTTPS and fragment-free. The policy is case-insensitive
  for the scheme (RFC 3986).
- `import_text`: arbitrary UTF-8 into the JSON, archive-JS and CSV entry points.
  Malformed input must return an error, not panic. This target found F-5, the
  archive-JS range panic recorded in `verification/FINDINGS.md`.

CI runs a 5 s-per-target smoke step in the `verification` job. A crash writes to
`fuzz/artifacts/` and fails the job; the input is then minimized with
`cargo fuzz tmin` and added to the regression tests before the fix.
