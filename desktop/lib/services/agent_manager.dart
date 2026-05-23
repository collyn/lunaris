import 'dart:io';
import 'dart:convert';
import 'package:flutter/foundation.dart';

class AgentManager {
  static final AgentManager _instance = AgentManager._internal();
  factory AgentManager() => _instance;
  AgentManager._internal();

  Process? _agentProcess;
  bool _isRunning = false;
  final List<String> _logs = [];
  VoidCallback? _onLogAdded;
  VoidCallback? _onStateChanged;

  bool get isRunning => _isRunning;
  List<String> get logs => List.unmodifiable(_logs);

  void setListeners({VoidCallback? onLogAdded, VoidCallback? onStateChanged}) {
    _onLogAdded = onLogAdded;
    _onStateChanged = onStateChanged;
  }

  /// Locate the agent binary in the workspace or bundle directory
  Future<String?> _findAgentBinary() async {
    final exePath = Platform.resolvedExecutable;
    final exeFile = File(exePath);
    final bundleDir = exeFile.parent;

    // 1. Try bundle directory (Production/Installed packaging)
    final prodAgent = File('${bundleDir.path}/agent');
    if (await prodAgent.exists()) {
      return prodAgent.path;
    }

    // 2. Try development workspace relative paths (target/debug/agent or target/release/agent)
    // Walk up from build directory to find workspace root
    Directory current = bundleDir;
    for (int i = 0; i < 7; i++) {
      final debugAgent = File('${current.path}/target/debug/agent');
      if (await debugAgent.exists()) {
        return debugAgent.path;
      }
      final releaseAgent = File('${current.path}/target/release/agent');
      if (await releaseAgent.exists()) {
        return releaseAgent.path;
      }
      current = current.parent;
    }

    return null;
  }

  /// Start the background agent mode
  Future<void> start({
    required String serverUrl,
    required String hostName,
    required String token,
  }) async {
    if (_isRunning) return;

    final agentPath = await _findAgentBinary();
    if (agentPath == null) {
      _addLog("ERROR: Rust agent binary not found in bundle or target build directory.");
      return;
    }

    final agentDir = File(agentPath).parent.path;
    final configPath = "$agentDir/agent_config.json";
    
    _addLog("Writing agent config to $configPath");
    try {
      final configObj = {
        "client_unique_id": "",
        "client_private_key": "",
        "client_certificate": "",
        "server_certificate": "",
        "server_url": serverUrl.startsWith('http') ? serverUrl : 'http://$serverUrl',
        "server_token": token
      };
      
      final configFile = File(configPath);
      if (!await configFile.exists()) {
        await configFile.writeAsString(jsonEncode(configObj));
      } else {
        try {
          final content = await configFile.readAsString();
          final Map<String, dynamic> existing = jsonDecode(content);
          existing["server_url"] = serverUrl.startsWith('http') ? serverUrl : 'http://$serverUrl';
          existing["server_token"] = token;
          await configFile.writeAsString(jsonEncode(existing));
        } catch (e) {
          await configFile.writeAsString(jsonEncode(configObj));
        }
      }
    } catch (e) {
      _addLog("Warning: failed to prepare agent_config.json: $e");
    }

    _addLog("Spawning Rust Agent daemon: $agentPath");
    try {
      // Start the agent as a CLI service (headless)
      _agentProcess = await Process.start(
        agentPath,
        [
          "--cli",
          "--config",
          configPath,
          "--name",
          hostName,
        ],
        environment: {
          "RUST_LOG": "info,agent=debug",
        },
      );

      _isRunning = true;
      _onStateChanged?.call();

      // Listen to stdout
      _agentProcess!.stdout.transform(utf8.decoder).listen((data) {
        for (var line in data.split('\n')) {
          if (line.trim().isNotEmpty) {
            _addLog("[Agent] $line");
          }
        }
      });

      // Listen to stderr
      _agentProcess!.stderr.transform(utf8.decoder).listen((data) {
        for (var line in data.split('\n')) {
          if (line.trim().isNotEmpty) {
            _addLog("[Agent Warn] $line");
          }
        }
      });

      // Handle process exit
      _agentProcess!.exitCode.then((code) {
        _addLog("Agent daemon exited with code $code");
        _cleanup();
      });

    } catch (e) {
      _addLog("ERROR spawning agent: $e");
      _cleanup();
    }
  }

  /// Stop the background agent mode
  Future<void> stop() async {
    if (!_isRunning) return;
    _addLog("Terminating Rust Agent daemon...");
    
    // Attempt graceful termination
    _agentProcess?.kill(ProcessSignal.sigterm);
    
    // Wait briefly and force kill if still running
    await Future.delayed(const Duration(seconds: 1));
    _agentProcess?.kill(ProcessSignal.sigkill);
    
    _cleanup();
  }

  void _cleanup() {
    _agentProcess = null;
    _isRunning = false;
    _onStateChanged?.call();
  }

  void _addLog(String msg) {
    final timestamp = DateTime.now().toIso8601String().substring(11, 19);
    _logs.add("[$timestamp] $msg");
    if (_logs.length > 500) {
      _logs.removeAt(0);
    }
    _onLogAdded?.call();
  }
}
