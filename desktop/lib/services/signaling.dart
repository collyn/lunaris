import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

class SignalingService {
  WebSocketChannel? _channel;
  RTCPeerConnection? _peerConnection;
  Function(String)? _onLog;
  Function(String)? _onStatusChange;
  Function(List<dynamic>, int?)? _onAppList;
  Function(MediaStream)? _onRemoteStream;
  final Map<String, RTCDataChannel> dataChannels = {};
  bool _isDisposed = false;

  SignalingService({
    Function(String)? onLog,
    Function(String)? onStatusChange,
    Function(List<dynamic>, int?)? onAppList,
    Function(MediaStream)? onRemoteStream,
  })  : _onLog = onLog,
        _onStatusChange = onStatusChange,
        _onAppList = onAppList,
        _onRemoteStream = onRemoteStream;

  void log(String msg) => _onLog?.call(msg);
  void setStatus(String status) => _onStatusChange?.call(status);

  RTCPeerConnection? get peerConnection => _peerConnection;

  /// Connect to WebSocket Signaling Server and initiate session or query app list
  Future<void> connect({
    required String serverHost, // e.g. "localhost:8080"
    required String token,
    required String hostId,
    int? appId,
    String? resolution, // "1080p", "720p", "540p"
    int? fps,
    int? bitrate,
    String? codec,
  }) async {
    final protocol = serverHost.startsWith('localhost') ? 'ws' : 'ws'; 
    // Secure websocket if https/wss is needed can be handled, but ws/http is fine.
    final wsUrl = '$protocol://$serverHost/ws/client?token=$token';

    log("Connecting to signaling server at $wsUrl");
    setStatus("Connecting...");

    try {
      _channel = WebSocketChannel.connect(Uri.parse(wsUrl));
      
      _channel!.stream.listen(
        (message) => _onMessageReceived(message, hostId, appId, resolution, fps, bitrate, codec),
        onError: (err) {
          log("WebSocket Error: $err");
          setStatus("Connection Error");
        },
        onDone: () {
          log("WebSocket connection closed.");
          setStatus("Disconnected");
        },
      );

      // On connection open, send GetAppList or RequestSession
      setStatus("Querying apps...");
      if (appId == null) {
        _send({
          "event": "Signaling",
          "data": {
            "type": "GetAppList",
            "payload": {"target_id": hostId}
          }
        });
        log("Sent GetAppList request for host $hostId");
      } else {
        _startSession(hostId, appId, resolution, fps, bitrate, codec);
      }

    } catch (e) {
      log("Error connecting to signaling: $e");
      setStatus("Failed to connect");
    }
  }

  void _send(dynamic data) {
    _channel?.sink.add(jsonEncode(data));
  }

  void _onMessageReceived(
    dynamic message,
    String hostId,
    int? appId,
    String? resolution,
    int? fps,
    int? bitrate,
    String? codec,
  ) async {
    try {
      final data = jsonDecode(message);
      if (data["event"] != "Signaling") return;

      final type = data["data"]["type"];
      final payload = data["data"]["payload"];

      switch (type) {
        case "AppListResponse":
          log("Received App list response.");
          final apps = payload["apps"] as List<dynamic>;
          final currentGameId = payload["current_game_id"] as int?;
          _onAppList?.call(apps, currentGameId);
          setStatus("Select App");
          break;

        case "Sdp":
          if (payload["sdp"]["ty"] == "offer") {
            log("Received SDP Offer from host agent.");
            _handleSDPOffer(payload["sdp"]["sdp"], hostId);
          }
          break;

        case "IceCandidate":
          log("Received ICE candidate from signaling.");
          _handleRemoteCandidate(payload["candidate"]);
          break;

        case "Error":
          log("Error from server: ${payload["message"]}");
          setStatus("Error: ${payload["message"]}");
          break;
        
        default:
          log("Unhandled signaling event type: $type");
      }
    } catch (e) {
      log("Error parsing incoming message: $e");
    }
  }

  /// Start streaming session
  void _startSession(
    String hostId,
    int appId,
    String? resolution,
    int? fps,
    int? bitrate,
    String? codec,
  ) async {
    setStatus("Establishing WebRTC...");
    
    int width = 1920;
    int height = 1080;
    if (resolution == '720p') {
      width = 1280;
      height = 720;
    } else if (resolution == '540p') {
      width = 960;
      height = 540;
    }

    final resolvedCodec = codec ?? 'h264';
    final resolvedFps = fps ?? 60;
    final resolvedBitrate = bitrate ?? 8000;

    log("Requesting WebRTC session: App=$appId, Size=${width}x$height, FPS=$resolvedFps, Bitrate=$resolvedBitrate, Codec=$resolvedCodec");

    // Initialize the WebRTC peer connection
    await _initPeerConnection(hostId);

    _send({
      "event": "Signaling",
      "data": {
        "type": "RequestSession",
        "payload": {
          "host_id": hostId,
          "width": width,
          "height": height,
          "fps": resolvedFps,
          "bitrate": resolvedBitrate,
          "codec": resolvedCodec,
          "app_id": appId,
        }
      }
    });
    log("Sent RequestSession command.");
  }

  /// Create WebRTC PeerConnection
  Future<void> _initPeerConnection(String hostId) async {
    final Map<String, dynamic> rtcConfig = {
      "iceServers": [
        {"urls": "stun:stun.l.google.com:19302"},
        {"urls": "stun:stun1.l.google.com:19302"}
      ],
      "sdpSemantics": "unified-plan"
    };

    _peerConnection = await createPeerConnection(rtcConfig);

    // Track state change
    _peerConnection!.onSignalingState = (state) {
      log("WebRTC Signaling State: $state");
    };

    _peerConnection!.onIceGatheringState = (state) {
      log("WebRTC ICE Gathering State: $state");
    };

    _peerConnection!.onConnectionState = (state) {
      log("WebRTC Connection State: $state");
      if (state == RTCPeerConnectionState.RTCPeerConnectionStateConnected) {
        setStatus("Streaming");
      } else if (state == RTCPeerConnectionState.RTCPeerConnectionStateFailed) {
        setStatus("Stream Failed");
      }
    };

    // When remote media track is received, mount the stream
    _peerConnection!.onAddStream = (MediaStream stream) {
      log("WebRTC Media stream track added.");
      _onRemoteStream?.call(stream);
    };

    // Send local ICE candidates to signaling
    _peerConnection!.onIceCandidate = (RTCIceCandidate candidate) {
      if (candidate.candidate != null) {
        _send({
          "event": "Signaling",
          "data": {
            "type": "IceCandidate",
            "payload": {
              "target_id": hostId,
              "candidate": {
                "candidate": candidate.candidate,
                "sdp_mid": candidate.sdpMid,
                "sdp_mline_index": candidate.sdpMLineIndex,
                "username_fragment": null
              }
            }
          }
        });
      }
    };

    // Listen for data channels created by the agent
    _peerConnection!.onDataChannel = (RTCDataChannel channel) {
      log("Data Channel established: ${channel.label}");
      dataChannels[channel.label!] = channel;
      
      channel.onDataChannelState = (state) {
        log("Data Channel ${channel.label} state: $state");
      };
      
      channel.onMessage = (RTCDataChannelMessage msg) {
        // We don't expect messages from host, but log them for debugging if any
      };
    };
  }

  /// Send input bytes over a specified data channel
  void sendInput(String channelLabel, List<int> bytes) {
    final channel = dataChannels[channelLabel];
    if (channel != null && channel.state == RTCDataChannelState.RTCDataChannelOpen) {
      channel.send(RTCDataChannelMessage.fromBinary(Uint8List.fromList(bytes)));
    }
  }

  /// Handle SDP Offer
  void _handleSDPOffer(String sdpText, String hostId) async {
    if (_peerConnection == null) return;
    log("Setting remote description...");

    try {
      await _peerConnection!.setRemoteDescription(RTCSessionDescription(sdpText, "offer"));
      
      log("Creating local answer description...");
      RTCSessionDescription answer = await _peerConnection!.createAnswer();
      
      log("Setting local description...");
      await _peerConnection!.setLocalDescription(answer);

      _send({
        "event": "Signaling",
        "data": {
          "type": "Sdp",
          "payload": {
            "target_id": hostId,
            "sdp": {
              "ty": "answer",
              "sdp": answer.sdp,
            }
          }
        }
      });
      log("Sent SDP Answer back to signaling.");
    } catch (e) {
      log("Failed to complete SDP Handshake: $e");
      setStatus("Handshake Failed");
    }
  }

  /// Handle remote ICE candidate
  void _handleRemoteCandidate(Map<String, dynamic> candidateData) async {
    if (_peerConnection == null) return;
    try {
      final candidate = RTCIceCandidate(
        candidateData["candidate"],
        candidateData["sdp_mid"],
        candidateData["sdp_mline_index"],
      );
      await _peerConnection!.addCandidate(candidate);
      log("Added remote ICE candidate.");
    } catch (e) {
      log("Error adding remote ICE candidate: $e");
    }
  }

  /// Stop active stream session
  void stopActiveStream(String hostId) {
    _send({
      "event": "Signaling",
      "data": {
        "type": "StopActiveStream",
        "payload": {
          "host_id": hostId,
        }
      }
    });
    log("Sent StopActiveStream command.");
  }

  /// Close and cleanup all resources
  Future<void> dispose() async {
    if (_isDisposed) return;
    _isDisposed = true;
    log("Disposing signaling connection and WebRTC...");

    // Nullify WebRTC callbacks synchronously to prevent background thread callbacks
    if (_peerConnection != null) {
      _peerConnection!.onSignalingState = null;
      _peerConnection!.onIceGatheringState = null;
      _peerConnection!.onConnectionState = null;
      _peerConnection!.onAddStream = null;
      _peerConnection!.onIceCandidate = null;
      _peerConnection!.onDataChannel = null;
    }

    for (var ch in dataChannels.values) {
      ch.onDataChannelState = null;
      ch.onMessage = null;
    }

    _channel?.sink.close();
    _channel = null;

    // Close data channels
    for (var ch in dataChannels.values) {
      try {
        await ch.close();
      } catch (e) {
        log("Error closing data channel: $e");
      }
    }
    dataChannels.clear();

    // Close peer connection
    if (_peerConnection != null) {
      try {
        await _peerConnection!.close();
      } catch (e) {
        log("Error closing peer connection: $e");
      }
      _peerConnection = null;
    }
  }
}
