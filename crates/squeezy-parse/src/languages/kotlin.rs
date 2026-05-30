use crate::ParsedFile;
use squeezy_workspace::FileRecord;
use tree_sitter::Tree;

pub(crate) fn extract_kotlin(file: FileRecord, _source: &str, _tree: &Tree) -> ParsedFile {
    ParsedFile::unsupported(file, "kotlin extractor not yet implemented")
}
