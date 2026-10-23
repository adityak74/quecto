cat > utils.py <<'E'
def slugify(s):
    raise NotImplementedError
E
cat > SPEC.md <<'E'
slugify(s): lowercase the text, replace every run of non-alphanumeric characters with a single "-", and strip leading and trailing "-".
E
