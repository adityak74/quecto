import re
with open('api.py') as f:
    content = f.read()
match = re.search(r'"""(.*)"""', content)
if match:
    with open('docs.md', 'w') as out:
        out.write(match.group(1))
