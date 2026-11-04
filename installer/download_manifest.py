"""Website-only schema. The updater's latest.json remains unchanged."""
from datetime import date
import json
from package import asset

# Platform keys are the public website contract, independent of Rust triples.
INSTALLERS = {
    'windows-x64': ('Windows x64', 'Chuchu-Setup-{version}-windows-x64.exe'),
    'macos-arm64': ('macOS Apple Silicon', 'Chuchu-{version}-aarch64-apple-darwin.pkg'),
    'macos-x64': ('macOS Intel', 'Chuchu-{version}-x86_64-apple-darwin.pkg'),
    'linux-x64': ('Linux x64 GNU', 'install-chuchu-{version}-x86_64-unknown-linux-gnu.sh'),
}


def generate(directory, version, base_url, release_date):
    if date.fromisoformat(release_date).isoformat() != release_date:
        raise ValueError('Release date must be YYYY-MM-DD')
    installers = {}
    for platform, (name, template) in INSTALLERS.items():
        path = directory / template.format(version=version)
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f'Missing or unsafe installer: {path.name}')
        installers[platform] = dict(name=name, filename=path.name, **asset(path, base_url))
    return dict(schema_version=1, version=version, release_date=release_date,
                installers=installers,
                release_notes_url=f'https://github.com/chuchu-lang/chuchu-core/releases/tag/v{version}')


def validate_downloads(directory, version, base_url):
    data = json.loads((directory / 'downloads.json').read_text())
    if (type(data.get('schema_version')) is not int
            or data != generate(directory, version, base_url, data['release_date'])):
        raise ValueError('Website manifest differs from installer files or release version')
    if (directory / f'downloads-{version}.json').read_bytes() != (directory / 'downloads.json').read_bytes():
        raise ValueError('Versioned website manifest differs from downloads.json')
    return data
