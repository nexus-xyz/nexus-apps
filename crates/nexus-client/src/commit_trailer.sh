# The commit-msg hook that a Nexus session adds through git config (hook.nexus-commit-trailer, git 2.54 and later).
# It marks a commit made in the session with a Co-authored-by trailer for Nexus, in place of the one Claude Code adds
# for Anthropic. GitHub keeps the trailer when it squashes a pull request, so Nexus can count the merged work that
# came from a Nexus session. Git runs it as a shell command with the trailer, then the message file. It never fails a
# commit, and it changes nothing when in doubt.
# No single quotes and no exclamation marks in this file: it travels quoted in GIT_CONFIG_PARAMETERS.
nexus_commit_trailer() {
  trailer="$1"
  message="$2"
  [ -n "$trailer" ] && [ -f "$message" ] || return 0
  # Only the repositories of the nexus-xyz organization: the remote a push goes to, as git resolves it, not every
  # remote, so a fork with nexus-xyz as its upstream stays unmarked. Its URL as git expands it (insteadOf), on any
  # host name, so an SSH alias of github.com counts.
  branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null)
  remote=$(git config --get "branch.$branch.pushRemote" 2>/dev/null || git config --get remote.pushDefault 2>/dev/null ||
    git config --get "branch.$branch.remote" 2>/dev/null || echo origin)
  case "$(git remote get-url --push "$remote" 2>/dev/null)" in
    *[:/][Nn][Ee][Xx][Uu][Ss]-[Xx][Yy][Zz]/*) ;;
    *) return 0 ;;
  esac
  # Only new work: not a merge, and not a commit that a rebase, a cherry-pick or a revert replays.
  case "${GIT_REFLOG_ACTION-}" in
    merge* | pull* | rebase* | cherry-pick* | revert*) return 0 ;;
  esac
  git rev-parse --git-path MERGE_HEAD --git-path CHERRY_PICK_HEAD --git-path REVERT_HEAD \
    --git-path rebase-merge --git-path rebase-apply 2>/dev/null |
    while IFS= read -r state; do if [ -e "$state" ]; then exit 1; fi; done || return 0
  # An empty message stops the commit: a trailer alone must not turn it into one. The diff below the scissors line
  # of `git commit -v` is not the message.
  body=$(
    while IFS= read -r line || [ -n "$line" ]; do
      # The leading parenthesis keeps this nested case valid in macOS /bin/sh.
      case "$line" in (*" ------------------------ >8 ------------------------") break ;; esac
      printf "%s\n" "$line"
    done <"$message" | git stripspace --strip-comments 2>/dev/null
  )
  [ -n "$body" ] || return 0
  # Nexus is the co-author, not the agent it ran: the trailer Claude Code adds for Anthropic goes. A person named as
  # a co-author stays.
  # A fixed path can point to another file. Create a private temporary file instead.
  kept=$(mktemp "$message.nexus.XXXXXX" 2>/dev/null) || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      [Cc][Oo]-[Aa][Uu][Tt][Hh][Oo][Rr][Ee][Dd]-[Bb][Yy]:*"<noreply@anthropic.com>"*) ;;
      *) printf "%s\n" "$line" ;;
    esac
  done <"$message" >"$kept" && mv -f "$kept" "$message" || rm -f "$kept"
  git interpret-trailers --in-place --no-divider --where end --if-exists addIfDifferent --if-missing add \
    --trailer "$trailer" "$message" >/dev/null 2>&1
  return 0
}
