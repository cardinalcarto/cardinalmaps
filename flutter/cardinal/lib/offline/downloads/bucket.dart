import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

/// A request to the bucket that didn't get what it asked for.
class BucketError implements Exception {
  const BucketError(this.key, this.status, [this.message]);

  /// The object, under `releases/`.
  final String key;

  /// The HTTP status, or null if there was no answer.
  final int? status;
  final String? message;

  /// The object changed since its ETag was read (`412` to `If-Match`).
  bool get changed => status == HttpStatus.preconditionFailed;

  @override
  String toString() => 'BucketError($key: ${status ?? message})';
}

/// Bytes of a range of an object, and the object's ETag.
typedef RangeBytes = ({Uint8List bytes, String? etag, int? total});

/// The releases in the bucket (`publish/README.md`), read in the foreground, while
/// planning and for the small JSON files a region keeps. Files a region downloads in
/// bulk go through the background downloader instead.
class Bucket {
  Bucket(this.base, {Duration timeout = const Duration(seconds: 30)})
    : _timeout = timeout {
    _client.connectionTimeout = timeout;
  }

  /// The bucket's `releases/`, such as `https://example.org/releases`.
  final Uri base;
  final Duration _timeout;
  final _client = HttpClient();

  Uri url(String key) =>
      base.replace(path: '${base.path.replaceFirst(RegExp(r'/$'), '')}/$key');

  void close() => _client.close(force: true);

  Future<HttpClientResponse> _send(
    String key, {
    Map<String, String> headers = const {},
    String method = 'GET',
  }) async {
    try {
      final request = await _client.openUrl(method, url(key)).timeout(_timeout);
      headers.forEach(request.headers.set);
      return await request.close().timeout(_timeout);
    } on TimeoutException {
      throw BucketError(key, null, 'timed out');
    } on IOException catch (e) {
      throw BucketError(key, null, '$e');
    }
  }

  Future<Uint8List> _body(String key, HttpClientResponse response) async {
    try {
      final builder = BytesBuilder(copy: false);
      await response.timeout(_timeout).forEach(builder.add);
      return builder.takeBytes();
    } on TimeoutException {
      throw BucketError(key, null, 'timed out');
    } on IOException catch (e) {
      throw BucketError(key, null, '$e');
    }
  }

  /// A JSON object, inflated if the bucket stored it gzipped.
  Future<Object?> json(String key) async {
    final response = await _send(key);
    final body = await _body(key, response);
    if (response.statusCode != HttpStatus.ok) {
      throw BucketError(key, response.statusCode);
    }
    try {
      return jsonDecode(utf8.decode(body));
    } on FormatException catch (e) {
      throw BucketError(key, response.statusCode, 'not JSON: $e');
    }
  }

  /// An object's size and the SHA-256 the bucket records for it (`x-amz-meta-sha256`),
  /// from a `HEAD`; null if there's no such object.
  Future<({int bytes, String? sha256})?> head(String key) async {
    final response = await _send(key, method: 'HEAD');
    await _body(key, response);
    if (response.statusCode == HttpStatus.notFound) return null;
    if (response.statusCode != HttpStatus.ok) {
      throw BucketError(key, response.statusCode);
    }
    final length = int.tryParse(
      response.headers.value(HttpHeaders.contentLengthHeader) ?? '',
    );
    if (length == null) throw BucketError(key, response.statusCode, 'no size');
    return (bytes: length, sha256: response.headers.value('x-amz-meta-sha256'));
  }

  /// [length] bytes of an object from [start], with `If-Match: ifMatch` if given.
  Future<RangeBytes> range(
    String key,
    int start,
    int length, {
    String? ifMatch,
  }) async {
    final response = await _send(
      key,
      headers: {
        'range': 'bytes=$start-${start + length - 1}',
        'if-match': ?ifMatch,
      },
    );
    final body = await _body(key, response);
    if (response.statusCode != HttpStatus.partialContent &&
        response.statusCode != HttpStatus.ok) {
      throw BucketError(key, response.statusCode);
    }
    final whole = response.statusCode == HttpStatus.ok;
    final total = RegExp(r'/(\d+)$')
        .firstMatch(response.headers.value('content-range') ?? '');
    final bytes = whole
        ? Uint8List.sublistView(
            body,
            start.clamp(0, body.length),
            (start + length).clamp(0, body.length),
          )
        : body;
    return (
      bytes: bytes,
      etag: response.headers.value('etag'),
      total: total == null
          ? (whole ? body.length : null)
          : int.parse(total[1]!),
    );
  }
}
