const fs = require('fs');
const path = require('path');

// --- Constants ---
const RELATIVE_LINK_REGEX = /\[.*\]\((\.\.\/.*)\)/g;
const INCLUDE = process.env.INPUT_INCLUDE || '';
const BASE_DIRECTORY = process.env.INPUT_BASE_DIRECTORY || process.env.GITHUB_WORKSPACE || process.cwd();
const IGNORE_PATHS = (process.env.INPUT_IGNORE_PATHS || '')
  .split(',')
  .filter(Boolean)
  .map((p) => path.resolve(p.trim()));
const DEBUG = process.env.DEBUG === 'true' || process.env.ACTIONS_RUNNER_DEBUG;



if (DEBUG) console.debug('Resolved Config:', { INCLUDE, BASE_DIRECTORY, IGNORE_PATHS });

// --- Functions ---
function processMarkdownLinks(content, filePath, baseDirectory, issues) {
  const matches = content.matchAll(RELATIVE_LINK_REGEX);

  for (const match of matches) {
    const relativePath = match[1];
    const resolvedPath = path.resolve(path.dirname(filePath), relativePath);

    const startIndex = match.index;
    const lineNumber = content.substring(0, startIndex).split('\n').length;
    const columnNumber = startIndex - content.lastIndexOf('\n', startIndex - 1);

    if (!resolvedPath.startsWith(baseDirectory)) {
      issues.push({
        file: filePath,
        issue: `Link points outside the base directory. "${resolvedPath}" is outside "${baseDirectory}".`,
        line: lineNumber,
        column: columnNumber
      });
      continue;
    }

    if (!fs.existsSync(resolvedPath)) {
      issues.push({
        file: filePath,
        issue: `Link points to a non-existent file. "${resolvedPath}" does not exist.`,
        line: lineNumber,
        column: columnNumber
      });
    }
  }
}

async function scanDirectory(directory, baseDirectory, issues = []) {
  const resolvedBasePath = path.resolve(baseDirectory);
  const include = path.resolve(resolvedBasePath, directory);
  if (DEBUG) console.debug(`Scanning directory: ${include}`);
  
  const files = fs.readdirSync(include);
  files.forEach((file) => {
    const filePath = path.join(include, file);

    // Skip ignored paths
    if (IGNORE_PATHS.some((ignorePath) => filePath.startsWith(ignorePath))) {
      if (DEBUG) console.debug(`Skipping ignored path: ${filePath}`);
      return;
    }

    const stat = fs.statSync(filePath);

    if (stat.isDirectory()) {
      return scanDirectory(filePath, baseDirectory, issues); // Recursively scan subdirectories
    }

    // Skip non-markdown and non-mdx files
    const extension = path.extname(file);
    if (!['.md', '.mdx'].includes(extension)) {
      return;
    }

    if (DEBUG) console.debug(`Processing file: ${filePath}`);
    const content = fs.readFileSync(filePath, 'utf8');
    processMarkdownLinks(content, filePath, baseDirectory, issues);
  });

  return issues;
}

// --- Main Function ---
function main() {
  scanDirectory(INCLUDE, BASE_DIRECTORY).then((issues) => {
    fs.appendFileSync(process.env.GITHUB_OUTPUT, `issues=${JSON.stringify(issues)}\n`);
    console.log('output.issues:', issues);
  }).catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

// --- Execute ---
main();
