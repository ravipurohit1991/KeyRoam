// Raw FFI binding to the Rust engine (crates/keyroam-ffi), bundled by
// hook/build.dart as the code asset `package:keyroam/keyroam_ffi`.

import 'dart:ffi';

import 'package:ffi/ffi.dart';

@Native<Pointer<Utf8> Function(Pointer<Utf8>)>(symbol: 'keyroam_call', assetId: 'package:keyroam/keyroam_ffi')
external Pointer<Utf8> _keyroamCall(Pointer<Utf8> request);

@Native<Void Function(Pointer<Utf8>)>(symbol: 'keyroam_free_string', assetId: 'package:keyroam/keyroam_ffi')
external void _keyroamFreeString(Pointer<Utf8> s);

/// Send one JSON request to the engine and return the JSON response.
///
/// Blocking; call it from a background isolate. The native copy of the
/// request (which may contain passwords) is wiped before it is freed; the
/// engine wipes its response buffer on free.
String invokeEngine(String requestJson) {
  final request = requestJson.toNativeUtf8(allocator: calloc);
  try {
    final response = _keyroamCall(request);
    try {
      return response.toDartString();
    } finally {
      _keyroamFreeString(response);
    }
  } finally {
    final bytes = request.cast<Uint8>();
    final length = request.length;
    for (var i = 0; i < length; i++) {
      bytes[i] = 0;
    }
    calloc.free(request);
  }
}
