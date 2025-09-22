Homebrew Packaging Guide

This repo includes a Homebrew formula template at `packaging/homebrew/repod.rb`. Follow these steps to make `brew install repod` work via your tap.

1) Pick distribution path
- Easiest: your own tap (recommended initially)
  - Create a GitHub repo named `homebrew-tap` under `iskng` (i.e., `iskng/homebrew-tap`).
  - Inside it, create `Formula/repod.rb` and copy the formula from `packaging/homebrew/repod.rb` (edit fields).
- Later: submit to `homebrew-core` once the project matures and meets their policies.

2) Tag a release
- Ensure `Cargo.toml` version matches your intended tag (currently `0.1.0`).
- Tag and push:
  git tag -a v0.1.0 -m "v0.1.0"
  git push origin v0.1.0

3) Fill in the formula
- Edit these fields in `Formula/repod.rb` (template already set to `iskng`):
  - `homepage` -> `https://github.com/iskng/repod`
  - `url` -> `https://github.com/iskng/repod/archive/refs/tags/v0.1.0.tar.gz`
  - `sha256` -> compute locally:
    curl -L -o repod-0.1.0.tar.gz https://github.com/iskng/repod/archive/refs/tags/v0.1.0.tar.gz
    shasum -a 256 repod-0.1.0.tar.gz | awk '{print $1}'
  - `license` -> MIT

Notes on dependencies
- The formula declares `rust` (build), `pkg-config` (build), and `openssl@3` because the `git2` crate typically links against OpenSSL.

4) Publish your tap
- In the `iskng/homebrew-tap` repo:
  mkdir -p Formula
  cp path/to/repod.rb Formula/
  git add Formula/repod.rb && git commit -m "repod 0.1.0" && git push

5) Install and test
- From your tap:
  brew tap iskng/tap
  brew install iskng/tap/repod
- Quick local test without a tap (from this repo):
  brew install --build-from-source packaging/homebrew/repod.rb

6) Auditing (optional, but useful)
- After pushing the formula to your tap:
  brew audit --new-formula yourusername/tap/repod

Submitting to homebrew-core (optional)
- Ensure you have a clear license (LICENSE file) and `license` in `Cargo.toml`.
- Provide a stable tag/release and keep `Cargo.lock` committed.
- Include a minimal `test do` (already provided) that doesn’t require network.
- Homebrew-core has additional policy and notability requirements; if not met, stick with your tap.

Optional polish (nice-to-have)
- Shell completions: Add `clap_complete` to generate completions and install them in the formula (bash, zsh, fish).
- Manpage: Generate and install a manpage in the formula.
- CI: Add a GitHub Action that builds and runs `repod -V` to sanity check releases.

Submodule + Automation Setup (Recommended)

Goal: Keep a dedicated `homebrew-tap` repo as a submodule here for convenience, while Homebrew still taps it as an independent repo.

1) Create the tap repo
- GitHub repo: `github.com/iskng/homebrew-tap` (public)
- Create `Formula/` and place a `repod.rb` initially (you can copy from `packaging/homebrew/repod.rb`).

2) Add as a submodule here
- Run:
  git submodule add git@github.com:iskng/homebrew-tap.git packaging/homebrew-tap
  git commit -m "Add homebrew tap submodule"
- Developers will clone with submodules:
  git clone --recurse-submodules git@github.com:<you>/repod.git

3) Configure CI to update the tap automatically
- This repo includes `.github/workflows/publish-tap.yml`.
- Add secrets to this repo:
  - `TAP_REPO`: `iskng/homebrew-tap`
  - `TAP_PUSH_TOKEN`: A PAT with `contents:write` permission on the tap repo
- On tag push (e.g., `v0.1.0`) or manual dispatch, CI will:
  - Compute the sha256 of the GitHub tag tarball
  - Write/update `Formula/repod.rb` in the tap repo
  - Commit and push to the tap

4) Release flow with submodule
- Tag here: `git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z`
- CI updates the tap automatically.
- Optionally, bump the submodule pointer in this repo to the latest tap commit:
  git submodule update --remote packaging/homebrew-tap
  git add packaging/homebrew-tap
  git commit -m "Bump tap submodule to vX.Y.Z"
  git push

Notes
- Homebrew must tap the external repo, not the submodule path of this repo.
- The formula builds from the source tarball for the tag; keep `Cargo.lock` committed and license set.
