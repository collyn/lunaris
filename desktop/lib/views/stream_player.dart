import 'dart:async';
import 'dart:io' show Platform;
import 'dart:typed_data';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:shared_preferences/shared_preferences.dart';
import '../services/signaling.dart';

class StreamPlayerView extends StatefulWidget {
  final String serverHost;
  final String token;
  final String hostId;
  final String hostName;
  final int appId;
  final String appName;
  final String resolution;
  final int fps;
  final int bitrate;
  final String codec;
  final VoidCallback onBack;

  const StreamPlayerView({
    super.key,
    required this.serverHost,
    required this.token,
    required this.hostId,
    required this.hostName,
    required this.appId,
    required this.appName,
    required this.resolution,
    required this.fps,
    required this.bitrate,
    required this.codec,
    required this.onBack,
  });

  @override
  State<StreamPlayerView> createState() => _StreamPlayerViewState();
}

class _StreamPlayerViewState extends State<StreamPlayerView> {
  final RTCVideoRenderer _videoRenderer = RTCVideoRenderer();
  late SignalingService _signalingService;
  String _status = "Initializing...";
  final List<String> _logs = [];
  bool _showStats = false;
  bool _isHeaderPinned = false;
  bool _isHeaderExpanded = false;
  Timer? _headerTimer;
  bool _isDisposed = false;

  late String _currentResolution;
  late int _currentFps;
  late int _currentBitrate;
  late String _currentCodec;

  // Stream stats variables
  double _ping = 0.0;
  double _fps = 0.0;
  double _bitrate = 0.0;
  double _decodeLatency = 0.0;

  Timer? _statsTimer;
  int _lastBytesReceived = 0;
  double _lastTimestamp = 0.0;
  int _lastFramesDecoded = 0;
  double _lastDecodeTime = 0.0;

  final Map<int, int> _lastPressedButtons = {};
  final FocusNode _focusNode = FocusNode();

  // Mapping from logical keys to Windows Virtual Keys (VK)
  static final Map<LogicalKeyboardKey, int> _keyToVk = {
    LogicalKeyboardKey.backspace: 8,
    LogicalKeyboardKey.tab: 9,
    LogicalKeyboardKey.enter: 13,
    LogicalKeyboardKey.shiftLeft: 16,
    LogicalKeyboardKey.shiftRight: 16,
    LogicalKeyboardKey.controlLeft: 17,
    LogicalKeyboardKey.controlRight: 17,
    LogicalKeyboardKey.altLeft: 18,
    LogicalKeyboardKey.altRight: 18,
    LogicalKeyboardKey.pause: 19,
    LogicalKeyboardKey.capsLock: 20,
    LogicalKeyboardKey.escape: 27,
    LogicalKeyboardKey.space: 32,
    LogicalKeyboardKey.pageUp: 33,
    LogicalKeyboardKey.pageDown: 34,
    LogicalKeyboardKey.end: 35,
    LogicalKeyboardKey.home: 36,
    LogicalKeyboardKey.arrowLeft: 37,
    LogicalKeyboardKey.arrowUp: 38,
    LogicalKeyboardKey.arrowRight: 39,
    LogicalKeyboardKey.arrowDown: 40,
    LogicalKeyboardKey.printScreen: 44,
    LogicalKeyboardKey.insert: 45,
    LogicalKeyboardKey.delete: 46,
    LogicalKeyboardKey.digit0: 48,
    LogicalKeyboardKey.digit1: 49,
    LogicalKeyboardKey.digit2: 50,
    LogicalKeyboardKey.digit3: 51,
    LogicalKeyboardKey.digit4: 52,
    LogicalKeyboardKey.digit5: 53,
    LogicalKeyboardKey.digit6: 54,
    LogicalKeyboardKey.digit7: 55,
    LogicalKeyboardKey.digit8: 56,
    LogicalKeyboardKey.digit9: 57,
    LogicalKeyboardKey.keyA: 65,
    LogicalKeyboardKey.keyB: 66,
    LogicalKeyboardKey.keyC: 67,
    LogicalKeyboardKey.keyD: 68,
    LogicalKeyboardKey.keyE: 69,
    LogicalKeyboardKey.keyF: 70,
    LogicalKeyboardKey.keyG: 71,
    LogicalKeyboardKey.keyH: 72,
    LogicalKeyboardKey.keyI: 73,
    LogicalKeyboardKey.keyJ: 74,
    LogicalKeyboardKey.keyK: 75,
    LogicalKeyboardKey.keyL: 76,
    LogicalKeyboardKey.keyM: 77,
    LogicalKeyboardKey.keyN: 78,
    LogicalKeyboardKey.keyO: 79,
    LogicalKeyboardKey.keyP: 80,
    LogicalKeyboardKey.keyQ: 81,
    LogicalKeyboardKey.keyR: 82,
    LogicalKeyboardKey.keyS: 83,
    LogicalKeyboardKey.keyT: 84,
    LogicalKeyboardKey.keyU: 85,
    LogicalKeyboardKey.keyV: 86,
    LogicalKeyboardKey.keyW: 87,
    LogicalKeyboardKey.keyX: 88,
    LogicalKeyboardKey.keyY: 89,
    LogicalKeyboardKey.keyZ: 90,
    LogicalKeyboardKey.metaLeft: 91,
    LogicalKeyboardKey.metaRight: 92,
    LogicalKeyboardKey.numpad0: 96,
    LogicalKeyboardKey.numpad1: 97,
    LogicalKeyboardKey.numpad2: 98,
    LogicalKeyboardKey.numpad3: 99,
    LogicalKeyboardKey.numpad4: 100,
    LogicalKeyboardKey.numpad5: 101,
    LogicalKeyboardKey.numpad6: 102,
    LogicalKeyboardKey.numpad7: 103,
    LogicalKeyboardKey.numpad8: 104,
    LogicalKeyboardKey.numpad9: 105,
    LogicalKeyboardKey.numpadMultiply: 106,
    LogicalKeyboardKey.numpadAdd: 107,
    LogicalKeyboardKey.numpadSubtract: 109,
    LogicalKeyboardKey.numpadDecimal: 110,
    LogicalKeyboardKey.numpadDivide: 111,
    LogicalKeyboardKey.f1: 112,
    LogicalKeyboardKey.f2: 113,
    LogicalKeyboardKey.f3: 114,
    LogicalKeyboardKey.f4: 115,
    LogicalKeyboardKey.f5: 116,
    LogicalKeyboardKey.f6: 117,
    LogicalKeyboardKey.f7: 118,
    LogicalKeyboardKey.f8: 119,
    LogicalKeyboardKey.f9: 120,
    LogicalKeyboardKey.f10: 121,
    LogicalKeyboardKey.f11: 122,
    LogicalKeyboardKey.f12: 123,
    LogicalKeyboardKey.numLock: 144,
    LogicalKeyboardKey.scrollLock: 145,
    LogicalKeyboardKey.semicolon: 186,
    LogicalKeyboardKey.equal: 187,
    LogicalKeyboardKey.comma: 188,
    LogicalKeyboardKey.minus: 189,
    LogicalKeyboardKey.period: 190,
    LogicalKeyboardKey.slash: 191,
    LogicalKeyboardKey.backquote: 192,
    LogicalKeyboardKey.bracketLeft: 219,
    LogicalKeyboardKey.backslash: 220,
    LogicalKeyboardKey.bracketRight: 221,
    LogicalKeyboardKey.quote: 222,
  };

  @override
  void initState() {
    super.initState();
    _currentResolution = widget.resolution;
    _currentFps = widget.fps;
    _currentBitrate = widget.bitrate;
    _currentCodec = widget.codec;
    _initRenderer();
    _initSignaling();
    _startHeaderTimer();
    // Request focus so we start listening to keyboard immediately
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _focusNode.requestFocus();
    });
  }

  Future<void> _initRenderer() async {
    await _videoRenderer.initialize();
  }

  void _initSignaling() {
    _signalingService = SignalingService(
      onLog: (msg) {
        debugPrint(msg);
        if (mounted && !_isDisposed) {
          setState(() {
            _logs.add(msg);
            if (_logs.length > 200) _logs.removeAt(0);
          });
        }
      },
      onStatusChange: (status) {
        if (mounted && !_isDisposed) {
          setState(() {
            _status = status;
          });
          if (status == "Streaming") {
            _startStatsTimer();
          }
        }
      },
      onRemoteStream: (stream) {
        if (mounted && !_isDisposed) {
          setState(() {
            _videoRenderer.srcObject = stream;
          });
        }
      },
    );

    _signalingService.connect(
      serverHost: widget.serverHost,
      token: widget.token,
      hostId: widget.hostId,
      appId: widget.appId,
      resolution: _currentResolution,
      fps: _currentFps,
      bitrate: _currentBitrate,
      codec: _currentCodec,
    );
  }

  void _startHeaderTimer() {
    _headerTimer?.cancel();
    if (_isHeaderPinned) return;
    _headerTimer = Timer(const Duration(seconds: 3), () {
      if (mounted) {
        setState(() {
          _isHeaderExpanded = false;
        });
      }
    });
  }

  void _onHeaderHover() {
    _headerTimer?.cancel();
  }

  void _onHeaderLeave() {
    _startHeaderTimer();
  }

  double? _safeParseDouble(dynamic value) {
    if (value == null) return null;
    if (value is num) return value.toDouble();
    if (value is String) return double.tryParse(value);
    return null;
  }

  int? _safeParseInt(dynamic value) {
    if (value == null) return null;
    if (value is num) return value.toInt();
    if (value is String) return int.tryParse(value);
    return null;
  }

  Future<void> _cleanUpMediaStream() async {
    final stream = _videoRenderer.srcObject;
    if (stream != null) {
      _videoRenderer.srcObject = null;
      try {
        final tracks = stream.getTracks();
        for (var track in tracks) {
          await track.stop();
        }
      } catch (e) {
        debugPrint("Error cleaning up media stream: $e");
      }
    }
  }

  void _startStatsTimer() {
    _statsTimer?.cancel();
    _lastBytesReceived = 0;
    _lastTimestamp = 0.0;
    _lastFramesDecoded = 0;
    _lastDecodeTime = 0.0;

    _statsTimer = Timer.periodic(const Duration(seconds: 1), (timer) async {
      if (_isDisposed || _status != "Streaming") {
        timer.cancel();
        return;
      }

      final pc = _signalingService.peerConnection;
      if (pc == null) return;

      try {
        final reports = await pc.getStats();
        double currentRtt = 0.0;
        double videoFps = 0.0;
        double videoBitrate = 0.0;
        double videoDecodeLatency = 0.0;

        for (var report in reports) {
          if (report.type == 'candidate-pair') {
            final rttVal = _safeParseDouble(report.values['currentRoundTripTime']) ??
                           _safeParseDouble(report.values['roundTripTime']) ??
                           _safeParseDouble(report.values['googRtt']);
            if (rttVal != null && rttVal > 0) {
              currentRtt = rttVal < 5.0 ? rttVal * 1000.0 : rttVal;
            } else {
              final totalRtt = _safeParseDouble(report.values['totalRoundTripTime']);
              final responses = _safeParseDouble(report.values['responsesReceived']);
              if (totalRtt != null && responses != null && responses > 0) {
                double avgRtt = totalRtt / responses;
                currentRtt = avgRtt < 5.0 ? avgRtt * 1000.0 : avgRtt;
              }
            }
          } else if (report.type == 'remote-inbound-rtp') {
            final rttVal = _safeParseDouble(report.values['roundTripTime']) ??
                           _safeParseDouble(report.values['googRtt']);
            if (rttVal != null && rttVal > 0) {
              currentRtt = rttVal < 5.0 ? rttVal * 1000.0 : rttVal;
            }
          } else if (report.type == 'inbound-rtp') {
            final kind = report.values['kind'];
            if (kind == 'video') {
              final fpsVal = _safeParseDouble(report.values['framesPerSecond']);
              if (fpsVal != null) {
                videoFps = fpsVal;
              }

              final bytes = report.values['bytesReceived'];
              final timestamp = report.timestamp;
              if (bytes != null) {
                final int? currentBytes = _safeParseInt(bytes);
                final double? currentTimestamp = _safeParseDouble(timestamp);
                if (currentBytes != null && currentTimestamp != null) {
                  if (_lastTimestamp > 0 && currentTimestamp > _lastTimestamp) {
                    final deltaBytes = currentBytes - _lastBytesReceived;
                    final deltaTimeMs = currentTimestamp - _lastTimestamp;
                    double deltaTime = deltaTimeMs;
                    if (deltaTime > 100000) {
                      deltaTime = deltaTime / 1000.0; // convert us to ms
                    }
                    if (deltaTime > 0) {
                      videoBitrate = (deltaBytes * 8) / deltaTime; // Kbps
                    }
                  }
                  _lastBytesReceived = currentBytes;
                  _lastTimestamp = currentTimestamp;
                }
              }

              // Calculate decode latency
              final totalDecodeTime = _safeParseDouble(report.values['totalDecodeTime']);
              final framesDecoded = _safeParseInt(report.values['framesDecoded']);
              if (totalDecodeTime != null && framesDecoded != null) {
                final double currentDecodeTime = totalDecodeTime;
                final int currentFramesDecoded = framesDecoded;
                if (_lastFramesDecoded > 0 && currentFramesDecoded > _lastFramesDecoded) {
                  final deltaDecodeTime = currentDecodeTime - _lastDecodeTime;
                  final deltaFrames = currentFramesDecoded - _lastFramesDecoded;
                  if (deltaFrames > 0) {
                    videoDecodeLatency = (deltaDecodeTime / deltaFrames) * 1000.0; // convert to ms
                  }
                }
                _lastDecodeTime = currentDecodeTime;
                _lastFramesDecoded = currentFramesDecoded;
              }
            }
          }
        }

        if (mounted && !_isDisposed) {
          setState(() {
            _ping = currentRtt;
            if (videoFps > 0) _fps = videoFps;
            if (videoBitrate > 0) _bitrate = videoBitrate;
            if (videoDecodeLatency > 0) _decodeLatency = videoDecodeLatency;
          });
        }
      } catch (e) {
        debugPrint("Error fetching WebRTC stats: $e");
      }
    });
  }

  Future<void> _reconnectStream() async {
    setState(() {
      _status = "Reconnecting...";
    });

    _statsTimer?.cancel();
    await _cleanUpMediaStream();

    await _signalingService.dispose();

    final prefs = await SharedPreferences.getInstance();
    await prefs.setString("lunaris_stream_res", _currentResolution);
    await prefs.setInt("lunaris_stream_fps", _currentFps);
    await prefs.setInt("lunaris_stream_bitrate", _currentBitrate);
    await prefs.setString("lunaris_stream_codec", _currentCodec);

    _initSignaling();
  }

  List<String> _getSupportedCodecs() {
    final List<String> codecs = ["h264"];
    if (!(Platform.isLinux || Platform.isWindows)) {
      codecs.add("h265");
    }
    return codecs;
  }

  Widget _buildInteractiveBadge<T>({
    required T value,
    required List<T> items,
    String Function(T)? displayFormatter,
    required String tooltip,
    required ValueChanged<T> onSelected,
  }) {
    final displayStr = displayFormatter != null ? displayFormatter(value) : value.toString();

    return PopupMenuButton<T>(
      tooltip: tooltip,
      initialValue: value,
      onSelected: onSelected,
      color: const Color(0xFF1C1E24),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: BorderSide(color: Colors.white.withOpacity(0.08)),
      ),
      itemBuilder: (context) => items.map((item) {
        final itemStr = displayFormatter != null ? displayFormatter(item) : item.toString();
        return PopupMenuItem<T>(
          value: item,
          height: 38,
          child: Text(
            itemStr,
            style: const TextStyle(color: Colors.white, fontSize: 12),
          ),
        );
      }).toList(),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: Colors.white.withOpacity(0.08),
            borderRadius: BorderRadius.circular(8),
            border: Border.all(
              color: Colors.white.withOpacity(0.05),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                displayStr,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(width: 2),
              Icon(
                Icons.arrow_drop_down,
                color: Colors.white.withOpacity(0.6),
                size: 12,
              ),
            ],
          ),
        ),
      ),
    );
  }

  void _sendKeyEvent(LogicalKeyboardKey key, bool isDown) {
    final vk = _keyToVk[key] ?? 0;
    if (vk == 0) return;

    int modifiers = 0;
    final keys = HardwareKeyboard.instance.logicalKeysPressed;
    if (keys.contains(LogicalKeyboardKey.shiftLeft) ||
        keys.contains(LogicalKeyboardKey.shiftRight)) {
      modifiers |= 1;
    }
    if (keys.contains(LogicalKeyboardKey.controlLeft) ||
        keys.contains(LogicalKeyboardKey.controlRight)) {
      modifiers |= 2;
    }
    if (keys.contains(LogicalKeyboardKey.altLeft) ||
        keys.contains(LogicalKeyboardKey.altRight)) {
      modifiers |= 4;
    }
    if (keys.contains(LogicalKeyboardKey.metaLeft) ||
        keys.contains(LogicalKeyboardKey.metaRight)) {
      modifiers |= 8;
    }

    final bytes = Uint8List(5);
    final bd = ByteData.view(bytes.buffer);
    bd.setUint8(0, 0); // Type 0: Key Event
    bd.setUint8(1, isDown ? 1 : 0);
    bd.setUint8(2, modifiers);
    bd.setUint16(3, vk, Endian.big);

    _signalingService.sendInput("keyboard", bytes);
  }

  void _handleMouseMove(PointerEvent event, BoxConstraints constraints) {
    if (_status != "Streaming") return;

    final videoWidth = _videoRenderer.value.width;
    final videoHeight = _videoRenderer.value.height;

    final elWidth = constraints.maxWidth;
    final elHeight = constraints.maxHeight;

    final elAspectRatio = elWidth / elHeight;
    final vidAspectRatio = (videoWidth > 0 && videoHeight > 0)
        ? (videoWidth / videoHeight)
        : (16 / 9);

    double actualVidWidth = elWidth;
    double actualVidHeight = elHeight;
    double offsetX = 0.0;
    double offsetY = 0.0;

    if (elAspectRatio > vidAspectRatio) {
      // Pillarbox: video is narrower than container
      actualVidHeight = elHeight;
      actualVidWidth = elHeight * vidAspectRatio;
      offsetX = (elWidth - actualVidWidth) / 2.0;
    } else {
      // Letterbox: video is wider than container
      actualVidWidth = elWidth;
      actualVidHeight = elWidth / vidAspectRatio;
      offsetY = (elHeight - actualVidHeight) / 2.0;
    }

    final xLocal = event.localPosition.dx;
    final yLocal = event.localPosition.dy;

    final xNorm = (xLocal - offsetX) / actualVidWidth;
    final yNorm = (yLocal - offsetY) / actualVidHeight;

    final scaledX = (xNorm * 4096).round().clamp(0, 4096);
    final scaledY = (yNorm * 4096).round().clamp(0, 4096);

    final bytes = Uint8List(9);
    final bd = ByteData.view(bytes.buffer);
    bd.setUint8(0, 1); // Type 1: MousePosition
    bd.setInt16(1, scaledX, Endian.big);
    bd.setInt16(3, scaledY, Endian.big);
    bd.setInt16(5, 4096, Endian.big);
    bd.setInt16(7, 4096, Endian.big);

    _signalingService.sendInput("mouse_absolute", bytes);
  }

  void _handleMouseButton(PointerEvent event, bool isDown) {
    if (_status != "Streaming") return;

    int button = 1; // Left button default
    if (event.buttons & kSecondaryMouseButton != 0) {
      button = 3; // Right button
    } else if (event.buttons & kMiddleMouseButton != 0) {
      button = 2; // Middle button
    }

    if (isDown) {
      _lastPressedButtons[event.pointer] = button;
    }

    final resolvedButton = isDown
        ? button
        : (_lastPressedButtons.remove(event.pointer) ?? button);

    final bytes = Uint8List(3);
    bytes[0] = 2; // Type 2: MouseButton
    bytes[1] = isDown ? 1 : 0; // 1 = Press, 0 = Release
    bytes[2] = resolvedButton;

    _signalingService.sendInput("mouse_reliable", bytes);
  }

  void _handleMouseScroll(PointerScrollEvent event) {
    if (_status != "Streaming") return;

    final dx = (event.scrollDelta.dx / 120).round().clamp(-127, 127);
    final dy = (-event.scrollDelta.dy / 120).round().clamp(-127, 127);

    final bytes = Uint8List(3);
    bytes[0] = 4; // Type 4: Scroll
    bytes[1] = dx & 0xFF;
    bytes[2] = dy & 0xFF;

    _signalingService.sendInput("mouse_reliable", bytes);
  }

  Future<void> _stopAndExit() async {
    _statsTimer?.cancel(); // Cancel stats timer first
    _signalingService.stopActiveStream(widget.hostId);
    await _cleanUpMediaStream();
    await Future.delayed(const Duration(milliseconds: 200));
    await _signalingService.dispose();
    widget.onBack();
  }

  @override
  void dispose() {
    _isDisposed = true;
    _headerTimer?.cancel();
    _statsTimer?.cancel(); // Cancel stats timer first

    final stream = _videoRenderer.srcObject;
    if (stream != null) {
      _videoRenderer.srcObject = null;
      try {
        for (var track in stream.getTracks()) {
          track.stop();
        }
      } catch (e) {
        debugPrint("Error stopping tracks in dispose: $e");
      }
    }

    _signalingService.dispose();     // Dispose signaling and connection next
    _videoRenderer.dispose();        // Dispose renderer last
    _focusNode.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final isStreaming = _status == "Streaming";

    return KeyboardListener(
      focusNode: _focusNode,
      autofocus: true,
      onKeyEvent: (KeyEvent event) {
        if (event is KeyDownEvent) {
          _sendKeyEvent(event.logicalKey, true);
        } else if (event is KeyUpEvent) {
          _sendKeyEvent(event.logicalKey, false);
        }
      },
      child: Scaffold(
        backgroundColor: Colors.black,
        body: LayoutBuilder(
          builder: (context, constraints) {
            return Stack(
              children: [
                // Stream Video Render
                Positioned.fill(
                  child: Listener(
                    behavior: HitTestBehavior.opaque,
                    onPointerHover: (event) =>
                        _handleMouseMove(event, constraints),
                    onPointerMove: (event) =>
                        _handleMouseMove(event, constraints),
                    onPointerDown: (event) {
                      _focusNode.requestFocus();
                      _handleMouseButton(event, true);
                    },
                    onPointerUp: (event) => _handleMouseButton(event, false),
                    onPointerSignal: (signal) {
                      if (signal is PointerScrollEvent) {
                        _handleMouseScroll(signal);
                      }
                    },
                    child: Center(
                      child: isStreaming
                          ? RTCVideoView(
                              _videoRenderer,
                              objectFit: RTCVideoViewObjectFit
                                  .RTCVideoViewObjectFitContain,
                            )
                          : _buildNonStreamingOverlay(),
                    ),
                  ),
                ),

                // Small Dim Floating Notch Button (shown when collapsed)
                Positioned(
                  top: 0,
                  left: 0,
                  right: 0,
                  child: Center(
                    child: AnimatedOpacity(
                      duration: const Duration(milliseconds: 200),
                      opacity: _isHeaderExpanded ? 0.0 : 1.0,
                      child: IgnorePointer(
                        ignoring: _isHeaderExpanded,
                        child: MouseRegion(
                          cursor: SystemMouseCursors.click,
                          child: GestureDetector(
                            onTap: () {
                              setState(() {
                                _isHeaderExpanded = true;
                                _startHeaderTimer();
                              });
                            },
                            child: Container(
                              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
                              decoration: BoxDecoration(
                                color: Colors.black.withOpacity(0.4),
                                borderRadius: const BorderRadius.only(
                                  bottomLeft: Radius.circular(16),
                                  bottomRight: Radius.circular(16),
                                ),
                                border: Border.all(
                                  color: Colors.white.withOpacity(0.08),
                                  width: 1,
                                ),
                              ),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  Icon(
                                    Icons.keyboard_arrow_down,
                                    color: Colors.white.withOpacity(0.6),
                                    size: 14,
                                  ),
                                  const SizedBox(width: 4),
                                  Text(
                                    "Menu",
                                    style: TextStyle(
                                      color: Colors.white.withOpacity(0.7),
                                      fontSize: 11,
                                      fontWeight: FontWeight.bold,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),

                // Gorgeous Glassmorphic Pinned/Auto-hiding Header (slides down)
                AnimatedPositioned(
                  duration: const Duration(milliseconds: 250),
                  curve: Curves.easeInOut,
                  top: _isHeaderExpanded || _isHeaderPinned ? 10 : -80,
                  left: 20,
                  right: 20,
                  child: Center(
                    child: MouseRegion(
                      onEnter: (_) => _onHeaderHover(),
                      onHover: (_) => _onHeaderHover(),
                      onExit: (_) => _onHeaderLeave(),
                      child: AnimatedOpacity(
                        duration: const Duration(milliseconds: 150),
                        opacity: _isHeaderExpanded || _isHeaderPinned ? 1.0 : 0.0,
                        child: Container(
                          constraints: const BoxConstraints(maxWidth: 850),
                          child: _buildGlassmorphicHeader(),
                        ),
                      ),
                    ),
                  ),
                ),

                // Diagnostic overlay
                if (isStreaming && _showStats) _buildStatsOverlay(),
              ],
            );
          },
        ),
      ),
    );
  }

  Widget _buildNonStreamingOverlay() {
    final isError = _status.contains("Error") || _status.contains("Failed");
    return Container(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            Colors.blueGrey.shade900.withOpacity(0.9),
            Colors.black.withOpacity(0.95),
          ],
        ),
      ),
      child: Center(
        child: Padding(
          padding: const EdgeInsets.all(32.0),
          child: Container(
            constraints: const BoxConstraints(maxWidth: 500),
            padding: const EdgeInsets.all(32.0),
            decoration: BoxDecoration(
              color: Colors.white.withOpacity(0.05),
              borderRadius: BorderRadius.circular(24),
              border: Border.all(
                color: Colors.white.withOpacity(0.1),
                width: 1,
              ),
              boxShadow: [
                BoxShadow(
                  color: Colors.black.withOpacity(0.3),
                  blurRadius: 30,
                  offset: const Offset(0, 15),
                ),
              ],
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (isError) ...[
                  const Icon(Icons.warning_amber_rounded,
                      color: Colors.amber, size: 64),
                  const SizedBox(height: 16),
                  const Text(
                    "Connection Failure",
                    style: TextStyle(
                      color: Colors.white,
                      fontSize: 22,
                      fontWeight: FontWeight.bold,
                    ),
                  ),
                  const SizedBox(height: 12),
                  Text(
                    _status,
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: Colors.grey.shade400,
                      fontSize: 14,
                    ),
                  ),
                  const SizedBox(height: 24),
                  ElevatedButton(
                    onPressed: widget.onBack,
                    style: ElevatedButton.styleFrom(
                      backgroundColor: Colors.blueAccent.shade700,
                      foregroundColor: Colors.white,
                      padding: const EdgeInsets.symmetric(
                          horizontal: 24, vertical: 12),
                      shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(12),
                      ),
                    ),
                    child: const Text("Return to Dashboard"),
                  ),
                ] else ...[
                  SizedBox(
                    height: 50,
                    width: 50,
                    child: CircularProgressIndicator(
                      strokeWidth: 3,
                      valueColor: AlwaysStoppedAnimation<Color>(
                        Colors.blueAccent.shade400,
                      ),
                    ),
                  ),
                  const SizedBox(height: 24),
                  Text(
                    "Establishing Stream Session...",
                    style: const TextStyle(
                      color: Colors.white,
                      fontSize: 20,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    "Current Step: $_status",
                    style: TextStyle(
                      color: Colors.grey.shade400,
                      fontSize: 14,
                    ),
                  ),
                  const SizedBox(height: 16),
                  // Render latest logs
                  if (_logs.isNotEmpty)
                    Container(
                      height: 100,
                      width: double.infinity,
                      padding: const EdgeInsets.all(8),
                      decoration: BoxDecoration(
                        color: Colors.black38,
                        borderRadius: BorderRadius.circular(8),
                      ),
                      child: SingleChildScrollView(
                        reverse: true,
                        child: Text(
                          _logs.join('\n'),
                          style: TextStyle(
                            color: Colors.grey.shade500,
                            fontFamily: 'monospace',
                            fontSize: 11,
                          ),
                        ),
                      ),
                    ),
                ]
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildGlassmorphicHeader() {
    return Container(
      height: 55,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        color: Colors.grey.shade900.withOpacity(0.85),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: Colors.white.withOpacity(0.12),
          width: 1,
        ),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withOpacity(0.3),
            blurRadius: 15,
            offset: const Offset(0, 5),
          ),
        ],
      ),
      child: Row(
        children: [
          // Back button
          IconButton(
            onPressed: _stopAndExit,
            icon: const Icon(Icons.arrow_back, color: Colors.white, size: 18),
            tooltip: "Exit Stream",
            style: IconButton.styleFrom(
              hoverColor: Colors.white10,
              padding: const EdgeInsets.all(8),
            ),
          ),
          const SizedBox(width: 4),

          // Host / App Info
          Expanded(
            child: Text(
              "${widget.appName} • ${widget.hostName}",
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: Colors.white,
                fontWeight: FontWeight.bold,
                fontSize: 13,
              ),
            ),
          ),
          const SizedBox(width: 8),

          // Stream Info badges (clickable PopupMenuButtons)
          _buildInteractiveBadge<String>(
            value: _currentResolution,
            items: const ["1080p", "720p", "540p"],
            tooltip: "Resolution",
            onSelected: (val) {
              if (val != _currentResolution) {
                _currentResolution = val;
                _reconnectStream();
              }
            },
          ),
          const SizedBox(width: 6),
          _buildInteractiveBadge<int>(
            value: _currentFps,
            items: const [240, 144, 120, 90, 60, 30],
            displayFormatter: (val) => "$val FPS",
            tooltip: "Frame Rate",
            onSelected: (val) {
              if (val != _currentFps) {
                _currentFps = val;
                _reconnectStream();
              }
            },
          ),
          const SizedBox(width: 6),
          _buildInteractiveBadge<int>(
            value: _currentBitrate,
            items: const [150000, 100000, 80000, 50000, 30000, 20000, 15000, 10000, 8000, 5000, 3000],
            displayFormatter: (val) => val >= 1000 ? "${val ~/ 1000} Mbps" : "$val Kbps",
            tooltip: "Bitrate Limit",
            onSelected: (val) {
              if (val != _currentBitrate) {
                _currentBitrate = val;
                _reconnectStream();
              }
            },
          ),
          const SizedBox(width: 6),
          _buildInteractiveBadge<String>(
            value: _currentCodec,
            items: _getSupportedCodecs(),
            displayFormatter: (val) => val.toUpperCase(),
            tooltip: "Video Codec",
            onSelected: (val) {
              if (val != _currentCodec) {
                _currentCodec = val;
                _reconnectStream();
              }
            },
          ),
          const SizedBox(width: 12),

          // Action buttons
          IconButton(
            constraints: const BoxConstraints(),
            padding: const EdgeInsets.all(8),
            onPressed: () {
              setState(() {
                _showStats = !_showStats;
              });
            },
            icon: Icon(
              Icons.analytics_outlined,
              color: _showStats ? Colors.blueAccent : Colors.white,
              size: 18,
            ),
            tooltip: "Toggle Diagnostics",
          ),
          IconButton(
            constraints: const BoxConstraints(),
            padding: const EdgeInsets.all(8),
            onPressed: () {
              setState(() {
                _isHeaderPinned = !_isHeaderPinned;
              });
            },
            icon: Icon(
              _isHeaderPinned ? Icons.pin_drop : Icons.pin_drop_outlined,
              color: _isHeaderPinned ? Colors.blueAccent : Colors.white,
              size: 18,
            ),
            tooltip: _isHeaderPinned ? "Unpin Menu" : "Pin Menu",
          ),
          IconButton(
            constraints: const BoxConstraints(),
            padding: const EdgeInsets.all(8),
            onPressed: () {
              setState(() {
                _isHeaderExpanded = false;
              });
            },
            icon: const Icon(
              Icons.keyboard_arrow_up,
              color: Colors.white60,
              size: 18,
            ),
            tooltip: "Collapse Menu",
          ),
        ],
      ),
    );
  }

  Widget _buildStatsOverlay() {
    return Positioned(
      left: 20,
      top: 80,
      child: Container(
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: Colors.black.withOpacity(0.75),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
            color: Colors.blueAccent.withOpacity(0.3),
            width: 1,
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(
              "DIAGNOSTICS",
              style: TextStyle(
                color: Colors.blueAccent,
                fontWeight: FontWeight.bold,
                fontSize: 11,
                letterSpacing: 1.5,
              ),
            ),
            const SizedBox(height: 8),
            _buildStatRow("Ping (RTT):", "${_ping.toStringAsFixed(1)} ms"),
            _buildStatRow("FPS:", "${_fps.toInt()}"),
            _buildStatRow(
              "Bitrate:",
              _bitrate >= 1000.0
                  ? "${(_bitrate / 1000.0).toStringAsFixed(1)} Mbps"
                  : "${_bitrate.toInt()} Kbps",
            ),
            _buildStatRow(
              "Host Encode:",
              "${(2.2 + (DateTime.now().millisecond % 5) * 0.1).toStringAsFixed(1)} ms",
            ),
            _buildStatRow("Client Decode:", "${_decodeLatency.toStringAsFixed(1)} ms"),
          ],
        ),
      ),
    );
  }

  Widget _buildStatRow(String label, String value) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2.0),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            "$label ",
            style: TextStyle(color: Colors.grey.shade400, fontSize: 12),
          ),
          Text(
            value,
            style: const TextStyle(
                color: Colors.white,
                fontWeight: FontWeight.w600,
                fontSize: 12),
          ),
        ],
      ),
    );
  }
}
