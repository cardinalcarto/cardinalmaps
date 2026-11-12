// A local stand-in for the release bucket: serves a `cardinal-publish --target dir:`
// layout over HTTP as the bucket does, with faults to inject.
import 'dart:convert';
import 'dart:io';

/// Serves [root] (holding `releases/` and `.meta/`): each object with the headers its
/// `.meta` file records (`Content-Type`, `Content-Encoding`, `x-amz-meta-sha256`), an
/// `ETag`, single byte ranges (`206`), `If-Match` (`412`), and `HEAD`.
class FakeBucket {
  FakeBucket._(this.root, this._server) {
    _server.listen(_serve);
  }

  static Future<FakeBucket> serve(String root) async => FakeBucket._(
    root,
    await HttpServer.bind(InternetAddress.loopbackIPv4, 0),
  );

  final String root;
  final HttpServer _server;

  /// `http://127.0.0.1:<port>/releases`.
  String get releasesUrl =>
      'http://${_server.address.host}:${_server.port}/releases';

  /// Requests by path (from `releases/`), `HEAD`s included.
  final requests = <String, int>{};

  /// `HEAD` requests by path.
  final heads = <String, int>{};

  /// Paths served with one byte changed, until removed.
  final corrupt = <String>{};

  /// Paths served with the byte at an offset changed, until removed.
  final corruptAt = <String, int>{};

  /// Paths whose next response that reaches the offset has the byte there changed.
  final corruptOnceAt = <String, int>{};

  /// Paths whose next response stops halfway through its body.
  final cutOnce = <String>{};

  /// Paths answered with `500` until removed.
  final failing = <String>{};

  /// When set, every object's ETag, as if they had all been replaced.
  String? etagOverride;

  Future<void> close() => _server.close(force: true);

  int requestsFor(bool Function(String path) test) => requests.entries
      .where((e) => test(e.key))
      .fold(0, (sum, e) => sum + e.value);

  Future<void> _serve(HttpRequest request) async {
    final response = request.response;
    try {
      final path = request.uri.path.replaceFirst(RegExp('^/'), '');
      final key = path.replaceFirst(RegExp('^releases/'), '');
      requests[key] = (requests[key] ?? 0) + 1;
      if (request.method == 'HEAD') heads[key] = (heads[key] ?? 0) + 1;
      final file = File('$root/$path');
      if (path.contains('..') || !await file.exists()) {
        response.statusCode = HttpStatus.notFound;
        return;
      }
      if (failing.contains(key)) {
        response.statusCode = HttpStatus.internalServerError;
        return;
      }
      final metaFile = File('$root/.meta/$path.json');
      final meta = await metaFile.exists()
          ? jsonDecode(await metaFile.readAsString()) as Map<String, Object?>
          : const <String, Object?>{};
      List<int> body = await file.readAsBytes();
      final etag = etagOverride ?? '"${meta['sha256'] ?? body.length}"';
      response.headers
        ..set('etag', etag)
        ..set('accept-ranges', 'bytes')
        ..contentType = ContentType.parse(
          meta['content_type'] as String? ?? 'application/octet-stream',
        );
      if (meta['content_encoding'] case final String encoding) {
        response.headers.set('content-encoding', encoding);
      }
      if (meta['sha256'] case final String sha) {
        response.headers.set('x-amz-meta-sha256', sha);
      }
      final ifMatch = request.headers.value('if-match');
      if (ifMatch != null && ifMatch != etag) {
        response.statusCode = HttpStatus.preconditionFailed;
        return;
      }
      if (corrupt.contains(key) && body.isNotEmpty) {
        body = List.of(body)..[body.length ~/ 2] ^= 0xff;
      }
      if (corruptAt[key] case final at? when at < body.length) {
        body = List.of(body)..[at] ^= 0xff;
      }
      final range = request.headers.value('range');
      if (range != null) {
        final match = RegExp(r'^bytes=(\d+)-(\d*)$').firstMatch(range);
        if (match == null) {
          response.statusCode = HttpStatus.requestedRangeNotSatisfiable;
          return;
        }
        final start = int.parse(match[1]!);
        final end = match[2]!.isEmpty
            ? body.length - 1
            : int.parse(match[2]!).clamp(0, body.length - 1);
        if (start > end) {
          response.statusCode = HttpStatus.requestedRangeNotSatisfiable;
          return;
        }
        response
          ..statusCode = HttpStatus.partialContent
          ..headers.set('content-range', 'bytes $start-$end/${body.length}');
        if (corruptOnceAt[key] case final at? when at >= start && at <= end) {
          corruptOnceAt.remove(key);
          body = List.of(body)..[at] ^= 0xff;
        }
        body = body.sublist(start, end + 1);
      }
      response.contentLength = body.length;
      if (request.method == 'HEAD') return;
      if (cutOnce.remove(key)) {
        final socket = await response.detachSocket();
        socket.add(body.sublist(0, body.length ~/ 2));
        await socket.flush();
        socket.destroy();
        return;
      }
      response.add(body);
    } finally {
      try {
        await response.close();
      } on Object {
        // Detached, or the client went away.
      }
    }
  }
}
