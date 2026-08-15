# Contributing

Thanks for helping! Keep it small, focused, and green.

## Process

1. Fork + branch: `fix/name`, `feat/name`.
2. Make the change — smallest diff that works.
3. Local checks (must all pass):
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --test gateway -- --test-threads=1
   ```
4. Open a PR with a one-paragraph summary + what you verified.

## Conventions

- **Security first**: token handling is constant-time; egress is
  deny-by-default; channel envelopes carry no credentials; body size caps on
  every decode path. If your change touches auth/routing, say how it stays
  within these rules.
- **SSH-signed commits** are welcome (`git commit -S`) but not required.
- Keep the admin UI on the token system (see `assets/app.css` `:root`).
- One concept per commit; no unrelated churn.

## Bugs / security issues

Open an issue. For security-sensitive findings, open a *private* issue or
email the maintainer directly — do not post credentials or live tokens.
