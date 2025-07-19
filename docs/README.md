# DBCrust Documentation

This is the source for the DBCrust documentation site.

## 🚀 GitHub Pages Setup

To enable GitHub Pages for this repository:

1. Go to **Settings** → **Pages** in your GitHub repository
2. Under **Source**, select **GitHub Actions**
3. Save the changes
4. Push any change to trigger the documentation deployment

The documentation will be available at: https://ctourriere.github.io/pgcrust/

## 📝 Local Development

To build and preview the documentation locally:

```bash
# Install dependencies
pip install mkdocs-material mkdocs-minify-plugin mkdocs-git-revision-date-localized-plugin

# Serve locally
mkdocs serve

# Build static site
mkdocs build
```

Visit http://127.0.0.1:8000/ to see the documentation.

## 📚 Documentation Structure

- `index.md` - Landing page
- `quick-start.md` - Getting started guide
- `installation.md` - Installation instructions
- `user-guide/` - User guides
- `python-api/` - Python API documentation
- `reference/` - Command reference
- `configuration.md` - Configuration guide