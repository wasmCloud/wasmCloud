# [[models]]

The `models` array contains a list of locations - on disk and via http servers - where smithy model files can be found. A codegen.toml file may contain several `[[models]]` sections, which should be located together, usually at the top of the file, since they represent input sources.

Model files must have either the Smithy IDL format, with a `.smithy` extension,
or the JSON-AST format, with a `.json` extension.

All model files must be loaded and parsed before any command may succeed.

## From file paths

When loading from files, the `[[models]]` entry contains a `path` and an optional `files` array.

`path` specifies a path to a file name or directory, and may be an absolute path, or relative to the folder containing codegen.toml.

`files`, if present, contains a list of files or folders, relative to the path.

Directories are scanned recursively for all model files ending in `.smithy` or `.json`.

Examples:

- single path
   ```text
   # Load a model from an absolute or relative path.
   # Relative paths are relative to the location of codegen.toml.
   [[models]]
   path = "models/foo.smithy"
   ```
  
 - folder 

   ```text
   # Search the my-models folder recursively, and load all model files found.
   # Path is absolute or relative
   [[models]]
   path = "my-models"
   ```
   
- path prefix with mix of folders and named files

   ```text
   # Load a list of files that share a common path prefix. This example
   # loads all the files in any subdirectory of /etc/models/v1/,
   # and the named files /etc/models/base/foo.smithy and /etc/models/base/bar.json
   [[models]]
   path = "/etc/models"
   files = [ "v1", "base/foo.smithy", "base/bar.json" ]
   ```

## From urls

When loading from urls, a `[[models]]` entry contains a `url` and an optional `files` array.
As with file path-based loading, `files` are relative to the url. There is no recursive search in the url variant.

Examples:

- single file url

  ```text
  # Load a single smithy file from a url
  [[models]]
  url = "https://example.com/models/foo.smithy"
  ```

- multiple files with common prefix
  
  ```text
  # Load multiple files from the same base url
  #   https://example.com/models/v1/foo.smithy and
  #   https://example.com/models/v1/bar.smithy"
  [[models]]
  url = "https://example.com/models/v1"
  files =  [ "foo.smithy", "bar.smithy" ]
  ```


## Caching

All files loaded by url are cached locally, to speed up development time
and to enable offline builds (after the first time).

Cached files are located in the following folders, depending on your platform.

|Platform | Value                               | Example                      |
| ------- | ----------------------------------- | ---------------------------- |
| Linux   | `$XDG_CACHE_HOME`/weld or `$HOME`/.cache/weld | /home/alice/.cache/weld           |
| macOS   | `$HOME`/Library/Caches/weld              | /Users/Alice/Library/Caches/weld  |
| Windows | `{FOLDERID_LocalAppData}\weld`           | C:\Users\Alice\AppData\Local\weld |


