//! Source identity and UTF-8 byte positions shared by diagnostics and debug metadata.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub line: u32,
    /// One-based UTF-8 byte column. Tabs occupy one byte, not a display tab stop.
    pub column: u32,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    path: PathBuf,
    text: String,
    line_starts: Vec<usize>,
    /// Disjoint byte ranges preserve file identity without changing public Span literals.
    origins: Vec<(usize, SourceFile)>,
}

impl SourceFile {
    pub fn new(path: PathBuf, text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(offset, _)| offset + 1));
        Self {
            path,
            text,
            line_starts,
            origins: Vec::new(),
        }
    }

    /// Build a compilation-wide source address space. Each file has a separator
    /// byte after it, so even its EOF position belongs unambiguously to that file.
    pub fn bundle(files: Vec<SourceFile>) -> Self {
        let path = files.first().map(|f| f.path.clone()).unwrap_or_default();
        let mut text = String::new();
        let mut origins = Vec::new();
        for file in files {
            origins.push((text.len(), file.clone()));
            text.push_str(file.text());
            text.push('\n');
        }
        let mut bundle = Self::new(path, text);
        bundle.origins = origins;
        bundle
    }

    /// Resolve a compilation-wide offset to the original file and local offset.
    pub fn origin(&self, offset: usize) -> Option<(&SourceFile, usize)> {
        if self.origins.is_empty() {
            return self.text.is_char_boundary(offset).then_some((self, offset));
        }
        let index = self.origins.partition_point(|(start, _)| *start <= offset);
        let (start, file) = self.origins.get(index.checked_sub(1)?)?;
        let local = offset - start;
        file.text.is_char_boundary(local).then_some((file, local))
    }

    pub fn files(&self) -> impl Iterator<Item = &SourceFile> {
        let single = self.origins.is_empty().then_some(self);
        single
            .into_iter()
            .chain(self.origins.iter().map(|(_, file)| file))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Reject invalid UTF-8 boundaries, out-of-range offsets and metadata overflow.
    pub fn position(&self, offset: usize) -> Option<SourcePosition> {
        if !self.origins.is_empty() {
            let (file, local) = self.origin(offset)?;
            return file.position(local);
        }
        if !self.text.is_char_boundary(offset) {
            return None;
        }
        let index = self.line_starts.partition_point(|start| *start <= offset) - 1;
        Some(SourcePosition {
            line: u32::try_from(index.checked_add(1)?).ok()?,
            column: u32::try_from(
                offset
                    .checked_sub(self.line_starts[index])?
                    .checked_add(1)?,
            )
            .ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_bytes_crlf_tabs_unicode_and_eof() {
        let file = SourceFile::new("test.chuchu".into(), "a\r\n\té\n".into());
        assert_eq!(
            file.position(0),
            Some(SourcePosition { line: 1, column: 1 })
        );
        assert_eq!(
            file.position(3),
            Some(SourcePosition { line: 2, column: 1 })
        );
        assert_eq!(
            file.position(4),
            Some(SourcePosition { line: 2, column: 2 })
        );
        assert_eq!(file.position(5), None);
        assert_eq!(
            file.position(6),
            Some(SourcePosition { line: 2, column: 4 })
        );
        assert_eq!(
            file.position(7),
            Some(SourcePosition { line: 3, column: 1 })
        );
        assert_eq!(file.position(8), None);
        let empty = SourceFile::new("empty.chuchu".into(), String::new());
        assert_eq!(
            empty.position(0),
            Some(SourcePosition { line: 1, column: 1 })
        );
    }
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    #[test]
    fn disjoint_ranges_preserve_eof_and_unicode_positions() {
        let bundle = SourceFile::bundle(vec![
            SourceFile::new("main.chuchu".into(), "é".into()),
            SourceFile::new("math.chuchu".into(), "\nx".into()),
        ]);
        assert_eq!(bundle.origin(2).unwrap().0.path(), Path::new("main.chuchu"));
        assert_eq!(bundle.origin(3).unwrap().0.path(), Path::new("math.chuchu"));
        assert_eq!(
            bundle.position(4),
            Some(SourcePosition { line: 2, column: 1 })
        );
        assert!(bundle.origin(1).is_none());
        assert!(bundle.origin(6).is_none());
    }
}
