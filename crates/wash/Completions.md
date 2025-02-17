# Shell completions

`wash` can generate scripts that enable auto-completion for several popular shells. With completion enabled,
you can press TAB while entering a `wash` command to see available options and
subcommands. (Depending on configuration, you may need to type a `-`, then TAB, to see options)

In the set-up instructions below, `wash` generates the completion script file when the shell starts, ensuring that you always 
have the latest version of the script even if wash was just updated.


## Zsh

Modify `~/.zshrc` by adding the following lines. The folder `$HOME/.wash` must be added to the `fpath` array before calling oh-my-zsh:

```
$HOME/.cargo/bin/wash completions -d $HOME/.wash zsh
fpath=( $HOME/.wash "${fpath[@]}" )
[ -n "$ZSH" ] && [ -r $ZSH/oh-my-zsh.sh ] && source $ZSH/oh-my-zsh.sh 
```

Completions will be enabled the next time you start a shell. Or, you can `source ~/.zshrc` for the completions to take effect in the current shell.


## Bash

Modify `~/.bashrc` by adding the following lines.

```
$HOME/.cargo/bin/wash completions -d $HOME/.wash bash
source $HOME/.wash/wash.bash 
```

Completions will be enabled the next time you start a shell. Or, you can `source ~/.bashrc` for the completions to take effect in the current shell.


## Fish

Modify `~/.fishrc` by adding the following lines.

```
mkdir -p ~/.config/fish/completions
$HOME/.cargo/bin/wash completions -d ~/.config/fish/completions fish
```


## PowerShell

Add the following line to your powershell profile script. This will generate `wash.ps1` in the specified folder.

```
wash completions -d "C:\Users\[User]\Documents\WindowsPowerShell" power-shell
```
