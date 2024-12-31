
### :warning: It looks like your commit is not formatted in line with Conventional Commits

This repository uses [Conventional Commits][cc] to enable automation and ensure consistent commit messages across the project.

### Errors

| SHA | Error | Commit Message |
| --- | ----- | -------------- |
${MD_ERRORS_ROWS}

### How to fix this issue

> [!NOTE]
> If you don't feel comfortable doing this, don't worry—a project maintainer will help correct this for you, before merging.

<details>

<summary>Expand for instructions</summary>

Please amend your commit message to follow the [Conventional Commits][cc] format. You can do this by running the following commands:

```
git rebase -i HEAD~${MD_ERRORS_COUNT}
```

This will open an editor with a list of commits. Mark the commit you want to amend with `edit`, save and close the editor. Then run:

```console
git commit --amend
```

This will open an editor with the commit message. Please update the commit message to follow the [Conventional Commits][cc] format. Save and close the editor.

Finally, run:

```console
git rebase --continue
```

This will continue the rebase process.

Finally, push your changes to your fork:

```console
git push --force-with-lease
```

</details>

[cc]: https://www.conventionalcommits.org/en/v1.0.0