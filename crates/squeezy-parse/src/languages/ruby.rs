use crate::ParsedFile;
use squeezy_workspace::FileRecord;
use tree_sitter::Tree;

pub(crate) fn extract_ruby(file: FileRecord, _source: &str, _tree: &Tree) -> ParsedFile {
    ParsedFile::unsupported(file, "ruby extractor not yet implemented")
}
