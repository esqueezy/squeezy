// Squeezy benchmark oracle for Dart corpora.
//
// First-PR shim: emits an empty oracle payload so the Rust bench wrapper can
// detect the helper, but actual element-model traversal is deferred to a
// follow-up PR (see `docs/internal/lang-specs/dart.md` §9). The full helper
// will use `package:analyzer`'s `AnalysisContextCollection` to walk
// `LibraryElement` / `ClassElement` / `MixinElement` / `ExtensionElement` /
// `ExtensionTypeElement` / `EnumElement` / `TopLevelFunctionElement` /
// `MethodElement` / `ConstructorElement` / `PropertyAccessorElement` /
// `FieldElement` / `TopLevelVariableElement` rows and emit them as JSON.

import 'dart:convert';
import 'dart:io';

void main(List<String> arguments) {
  final root = arguments.isNotEmpty ? arguments.first : '.';
  stderr.writeln('dart-oracle: scan root = $root');
  stderr.writeln(
      'dart-oracle: first-PR stub — full analyzer traversal deferred (see spec §9).');
  final payload = <String, Object>{
    'rows': <List<String>>[],
    'unparseable_files': <String>[],
    'status': 'Dart analyzer oracle stub; full traversal deferred to follow-up PR',
    'mode': 'scan-only',
  };
  stdout.writeln(jsonEncode(payload));
}
