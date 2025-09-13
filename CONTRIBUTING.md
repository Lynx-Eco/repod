Contributing to repod

Thanks for your interest in contributing! This project welcomes bug reports, feature requests, and pull requests.

How to contribute
- Discuss: Open an issue describing the bug or feature.
- Fork & branch: Create a branch from `master` for your change.
- Tests: If reasonable, add or update tests to cover your change.
- Style: Keep changes focused and consistent with the codebase.
- Commit messages: Use Conventional Commits (e.g., `feat:`, `fix:`, `chore:`).
- PR: Open a pull request and link any related issues.

Development
- Rust stable toolchain is recommended.
- Build: `cargo build`
- Lint: `cargo clippy` (if installed)
- Format: `cargo fmt` (if installed)

AI integration
- The `--commit` feature uses Google’s Generative Language API.
- Set `GEMINI_API_KEY` in your environment to try it locally.
- Be mindful of API costs and rate limits.

Security
- Do not include secrets (API keys, tokens) in issues or PRs.
- See SECURITY.md for how to report vulnerabilities.

Code of Conduct
- Be respectful and constructive. We follow common-sense community standards.

Thank you for helping improve repod!

