"""Remove YAML frontmatter from all markdown files in a directory."""

import os
import re
import sys


def remove_frontmatter(content: str) -> str:
    """Remove YAML frontmatter (--- ... ---) from the beginning of markdown content."""
    pattern = r'^---\s*\n.*?\n---\s*\n'
    return re.sub(pattern, '', content, count=1, flags=re.DOTALL)


def process_file(filepath: str) -> bool:
    """Process a single markdown file. Returns True if modified."""
    with open(filepath, 'r', encoding='utf-8') as f:
        content = f.read()

    if not content.startswith('---'):
        return False

    new_content = remove_frontmatter(content)
    if new_content == content:
        return False

    with open(filepath, 'w', encoding='utf-8') as f:
        f.write(new_content)
    return True


def main():
    target_dir = sys.argv[1] if len(sys.argv) > 1 else '.'
    count = 0

    for root, _, files in os.walk(target_dir):
        for name in files:
            if name.endswith('.md'):
                filepath = os.path.join(root, name)
                if process_file(filepath):
                    print(f'Processed: {filepath}')
                    count += 1

    print(f'\nDone. Removed frontmatter from {count} files.')


if __name__ == '__main__':
    main()
