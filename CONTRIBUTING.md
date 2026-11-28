# Contributing

## Commits

Every commit subject follows [Conventional Commits](https://www.conventionalcommits.org):

```
type(scope)!: summary
```

- `type`: `feat` `fix` `perf` `refactor` `docs` `test` `build` `ci` `chore` `revert`
- `scope` (optional): the area, lowercase, such as `core`, `caption`, `island`, `settings`, `apple`, `apple-bridge`, `whisper`, `bergamot`, `openai`, `speech-chunks`, `audio-coreaudio`, `audio-cpal`, `models`, `contract`, `ui`, `app`, `readme`, `deps`
- `!` marks a breaking change, such as a settings file an older build cannot read
- `summary`: starts lowercase, no full stop at the end; the whole subject is 72 characters at most

```
feat(caption): steady lines that never overlap
fix(core): a final translation never waits for a partial one
perf(speech-chunks): a slow engine hears waiting finals together
ci: cache Windows builds too
docs(readme): rewrite in English
refactor: remove code nothing uses
```

## Branches

`type/kebab-name`, with the same types: `feat/settings-sidebar`, `fix/stop-button`, `ci/windows-cache`, `docs/readme-english`.

## Pull requests

Every change reaches `main` through a pull request, merged with **squash**: the pull request title becomes the one commit on `main`, so the title follows the commit rule too. No local `git merge` into `main`.

`make conventions` checks the branch name and every commit subject since `origin/main`; `TITLE="..."` also checks a pull request title. CI runs it on every pull request.
