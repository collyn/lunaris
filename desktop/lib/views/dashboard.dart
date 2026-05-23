import 'dart:convert';
import 'dart:io' show Platform;
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:shared_preferences/shared_preferences.dart';
import '../services/agent_manager.dart';
import '../services/signaling.dart';
import 'stream_player.dart';

class DashboardView extends StatefulWidget {
  final String serverHost;
  final String token;
  final String username;
  final VoidCallback onLogout;

  const DashboardView({
    super.key,
    required this.serverHost,
    required this.token,
    required this.username,
    required this.onLogout,
  });

  @override
  State<DashboardView> createState() => _DashboardViewState();
}

class _DashboardViewState extends State<DashboardView> {
  List<dynamic> _hosts = [];
  bool _isLoadingHosts = false;
  String? _hostsError;

  // Pairing form state
  final _pairFormKey = GlobalKey<FormState>();
  final _nameController = TextEditingController();
  final _ipController = TextEditingController();
  final _sunshineUserController = TextEditingController();
  final _sunshinePassController = TextEditingController();
  bool _isPairing = false;
  String? _pairingError;
  bool _pairingSuccess = false;

  // Agent (Remote Control) status
  final AgentManager _agentManager = AgentManager();
  bool _allowRemoteControl = false;
  List<String> _agentLogs = [];

  // Stream Quality Settings (Persisted locally)
  String _streamRes = "1080p";
  int _streamFps = 60;
  int _streamBitrate = 8000;
  String _streamCodec = "h264";

  // Selected host for app query
  Map<String, dynamic>? _selectedHostForApps;
  List<dynamic> _hostApps = [];
  bool _isLoadingApps = false;
  SignalingService? _appQuerySignaling;
  StateSetter? _modalStateSetter;

  @override
  void initState() {
    super.initState();
    _loadStreamSettings();
    _fetchHosts();
    _initAgentStatus();
  }

  Future<void> _loadStreamSettings() async {
    final prefs = await SharedPreferences.getInstance();
    setState(() {
      _streamRes = prefs.getString("lunaris_stream_res") ?? "1080p";
      _streamFps = prefs.getInt("lunaris_stream_fps") ?? 60;
      _streamBitrate = prefs.getInt("lunaris_stream_bitrate") ?? 8000;
      _streamCodec = prefs.getString("lunaris_stream_codec") ?? "h264";
    });
  }

  Future<void> _saveStreamSettings() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString("lunaris_stream_res", _streamRes);
    await prefs.setInt("lunaris_stream_fps", _streamFps);
    await prefs.setInt("lunaris_stream_bitrate", _streamBitrate);
    await prefs.setString("lunaris_stream_codec", _streamCodec);
  }

  void _initAgentStatus() {
    _allowRemoteControl = _agentManager.isRunning;
    _agentLogs = List.from(_agentManager.logs);
    _agentManager.setListeners(
      onLogAdded: () {
        if (mounted) {
          setState(() {
            _agentLogs = List.from(_agentManager.logs);
          });
        }
      },
      onStateChanged: () {
        if (mounted) {
          setState(() {
            _allowRemoteControl = _agentManager.isRunning;
          });
        }
      },
    );
  }

  Future<void> _fetchHosts() async {
    if (_isLoadingHosts) return;
    setState(() {
      _isLoadingHosts = true;
      _hostsError = null;
    });

    try {
      final url = Uri.parse("http://${widget.serverHost}/api/hosts");
      final response = await http.get(url, headers: {
        "Authorization": "Bearer ${widget.token}",
      });

      if (response.statusCode == 200) {
        final data = jsonDecode(response.body);
        if (mounted) {
          setState(() {
            _hosts = data;
            _hostsError = null;
          });
        }
      } else if (response.statusCode == 401) {
        widget.onLogout();
      } else {
        setState(() {
          _hostsError = "Failed to load hosts: ${response.statusCode}";
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _hostsError = "Connection to signaling server failed.";
        });
      }
    } finally {
      if (mounted) {
        setState(() {
          _isLoadingHosts = false;
        });
      }
    }
  }

  Future<void> _pairHost() async {
    if (!_pairFormKey.currentState!.validate()) return;

    setState(() {
      _isPairing = true;
      _pairingError = null;
      _pairingSuccess = false;
    });

    try {
      final url = Uri.parse("http://${widget.serverHost}/api/hosts/pair");
      final response = await http.post(
        url,
        headers: {
          "Content-Type": "application/json",
          "Authorization": "Bearer ${widget.token}",
        },
        body: jsonEncode({
          "name": _nameController.text.trim(),
          "ip_address": _ipController.text.trim(),
          "sunshine_username": _sunshineUserController.text.trim(),
          "sunshine_password": _sunshineControllerPassword(),
        }),
      );

      final data = jsonDecode(response.body);

      if (response.statusCode == 201) {
        setState(() {
          _pairingSuccess = true;
          _nameController.clear();
          _ipController.clear();
          _sunshineUserController.clear();
          _sunshinePassController.clear();
        });
        _fetchHosts();
      } else {
        setState(() {
          _pairingError = data["error"] ?? "Failed to pair with host.";
        });
      }
    } catch (e) {
      setState(() {
        _pairingError = "Pairing request failed. Check server connection.";
      });
    } finally {
      setState(() {
        _isPairing = false;
      });
    }
  }

  String _sunshineControllerPassword() {
    return _sunshinePassController.text;
  }

  Future<void> _unpairHost(String hostId) async {
    final confirm = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: Colors.grey.shade900,
        title: const Text("Remove Host", style: TextStyle(color: Colors.white)),
        content: const Text(
          "Are you sure you want to unpair and remove this host?",
          style: TextStyle(color: Colors.grey),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text("Cancel"),
          ),
          ElevatedButton(
            onPressed: () => Navigator.of(context).pop(true),
            style: ElevatedButton.styleFrom(backgroundColor: Colors.red),
            child: const Text("Remove"),
          ),
        ],
      ),
    );

    if (confirm != true) return;

    try {
      final url = Uri.parse("http://${widget.serverHost}/api/hosts/$hostId");
      final response = await http.delete(
        url,
        headers: {
          "Authorization": "Bearer ${widget.token}",
        },
      );

      if (response.statusCode == 200) {
        _fetchHosts();
      } else {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text("Failed to remove host")),
        );
      }
    } catch (e) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text("Connection error during removal")),
      );
    }
  }

  Future<void> _toggleAgent(bool value) async {
    if (value) {
      // 1. Fetch agent token from server
      try {
        final url = Uri.parse("http://${widget.serverHost}/api/agent/token");
        final response = await http.get(url, headers: {
          "Authorization": "Bearer ${widget.token}",
        });

        if (response.statusCode == 200) {
          final data = jsonDecode(response.body);
          final agentToken = data["token"];

          // Start Agent
          await _agentManager.start(
            serverUrl: widget.serverHost,
            hostName: widget.username,
            token: agentToken,
          );
        } else {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text("Failed to retrieve agent registration token")),
          );
        }
      } catch (e) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text("Failed to fetch agent token: $e")),
        );
      }
    } else {
      await _agentManager.stop();
    }
    setState(() {
      _allowRemoteControl = _agentManager.isRunning;
    });
  }

  void _queryAppsForHost(Map<String, dynamic> host) {
    setState(() {
      _selectedHostForApps = host;
      _isLoadingApps = true;
      _hostApps = [];
    });

    _appQuerySignaling = SignalingService(
      onLog: (m) => debugPrint("[AppQuery] $m"),
      onStatusChange: (status) {
        if (status.contains("Error") && mounted) {
          setState(() {
            _isLoadingApps = false;
            _selectedHostForApps = null;
          });
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text("App query error: $status")),
          );
        }
      },
      onAppList: (apps, currentGameId) {
        if (mounted) {
          setState(() {
            _hostApps = apps;
            _isLoadingApps = false;
          });
          _modalStateSetter?.call(() {});
        }
        // Cleanup this temp signaling channel
        _appQuerySignaling?.dispose();
        _appQuerySignaling = null;
      },
    );

    _appQuerySignaling!.connect(
      serverHost: widget.serverHost,
      token: widget.token,
      hostId: host["id"],
    );
  }

  void _launchApp(Map<String, dynamic> app) {
    final host = _selectedHostForApps;
    if (host == null) return;

    // Save active resolution/bitrate choices
    _saveStreamSettings();

    // Close app list popup
    setState(() {
      _selectedHostForApps = null;
    });

    // Dismiss the bottom sheet modal first
    Navigator.of(context).pop();

    // Navigate to Stream View
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (context) => StreamPlayerView(
          serverHost: widget.serverHost,
          token: widget.token,
          hostId: host["id"],
          hostName: host["name"],
          appId: app["id"],
          appName: app["title"],
          resolution: _streamRes,
          fps: _streamFps,
          bitrate: _streamBitrate,
          codec: _streamCodec,
          onBack: () {
            Navigator.of(context).pop();
            _fetchHosts();
          },
        ),
      ),
    );
  }

  @override
  void dispose() {
    _nameController.dispose();
    _ipController.dispose();
    _sunshineUserController.dispose();
    _sunshinePassController.dispose();
    _appQuerySignaling?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: const Color(0xFF0D0E12),
      body: Container(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: Alignment.topLeft,
            radius: 1.5,
            colors: [
              Colors.deepPurple.shade900.withOpacity(0.2),
              const Color(0xFF0D0E12),
            ],
          ),
        ),
        child: SafeArea(
          child: Column(
            children: [
              // Beautiful Header
              _buildDashboardHeader(),

              Expanded(
                child: Padding(
                  padding: const EdgeInsets.all(16.0),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      // Host Directory (Left Column)
                      Expanded(
                        flex: 3,
                        child: _buildHostDirectory(),
                      ),
                      const SizedBox(width: 16),

                      // Control Panel & Pairing Form (Right Column)
                      Expanded(
                        flex: 2,
                        child: Column(
                          children: [
                            // Agent mode switch card
                            _buildAgentCard(),
                            const SizedBox(height: 16),

                            // Pairing card
                            Expanded(
                              child: _buildPairingCard(),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildDashboardHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
      decoration: BoxDecoration(
        color: Colors.white.withOpacity(0.02),
        border: Border(
          bottom: BorderSide(color: Colors.white.withOpacity(0.06), width: 1),
        ),
      ),
      child: Row(
        children: [
          // Logo
          const Icon(Icons.blur_on_rounded, color: Colors.blueAccent, size: 36),
          const SizedBox(width: 12),
          const Text(
            "LUNARIS",
            style: TextStyle(
              color: Colors.white,
              fontWeight: FontWeight.w900,
              fontSize: 22,
              letterSpacing: 2,
            ),
          ),
          const Spacer(),

          // Connection info
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            decoration: BoxDecoration(
              color: Colors.white.withOpacity(0.05),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Row(
              children: [
                const Icon(Icons.cloud_queue_rounded, color: Colors.grey, size: 16),
                const SizedBox(width: 8),
                Text(
                  widget.serverHost,
                  style: const TextStyle(color: Colors.grey, fontSize: 13),
                ),
              ],
            ),
          ),
          const SizedBox(width: 16),

          // User / Logout
          Row(
            children: [
              const CircleAvatar(
                radius: 16,
                backgroundColor: Colors.blueAccent,
                child: Icon(Icons.person, color: Colors.white, size: 18),
              ),
              const SizedBox(width: 10),
              Text(
                widget.username,
                style: const TextStyle(
                  color: Colors.white,
                  fontWeight: FontWeight.bold,
                  fontSize: 14,
                ),
              ),
              const SizedBox(width: 8),
              IconButton(
                onPressed: widget.onLogout,
                icon: const Icon(Icons.logout, color: Colors.redAccent, size: 20),
                tooltip: "Log Out",
              )
            ],
          )
        ],
      ),
    );
  }

  Widget _buildHostDirectory() {
    return Container(
      padding: const EdgeInsets.all(20),
      decoration: BoxDecoration(
        color: Colors.white.withOpacity(0.03),
        borderRadius: BorderRadius.circular(24),
        border: Border.all(color: Colors.white.withOpacity(0.07)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Text(
                "Host Directory",
                style: TextStyle(
                  color: Colors.white,
                  fontWeight: FontWeight.bold,
                  fontSize: 18,
                ),
              ),
              const Spacer(),
              IconButton(
                onPressed: _fetchHosts,
                icon: const Icon(Icons.refresh, color: Colors.blueAccent),
                tooltip: "Refresh Directory",
              ),
            ],
          ),
          const SizedBox(height: 16),

          Expanded(
            child: _isLoadingHosts && _hosts.isEmpty
                ? const Center(child: CircularProgressIndicator())
                : _hostsError != null
                    ? Center(
                        child: Text(
                          _hostsError!,
                          style: const TextStyle(color: Colors.amber, fontSize: 14),
                        ),
                      )
                    : _hosts.isEmpty
                        ? _buildEmptyDirectory()
                        : ListView.builder(
                            itemCount: _hosts.length,
                            itemBuilder: (context, index) {
                              final host = _hosts[index];
                              return _buildHostCard(host);
                            },
                          ),
          ),
        ],
      ),
    );
  }

  Widget _buildEmptyDirectory() {
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(Icons.devices_other_rounded, size: 64, color: Colors.grey.shade700),
          const SizedBox(height: 16),
          const Text(
            "No streaming hosts registered yet.",
            style: TextStyle(color: Colors.grey, fontWeight: FontWeight.bold, fontSize: 16),
          ),
          const SizedBox(height: 8),
          Text(
            "Pair a host on the right to start streaming.",
            style: TextStyle(color: Colors.grey.shade600, fontSize: 13),
          ),
        ],
      ),
    );
  }

  Widget _buildHostCard(Map<String, dynamic> host) {
    final status = host["status"] as String;
    final isOnline = status == "Online";
    final isBusy = status == "Busy";

    Color statusColor = Colors.grey;
    if (isOnline) statusColor = Colors.greenAccent;
    if (isBusy) statusColor = Colors.amberAccent;

    return Container(
      margin: const EdgeInsets.only(bottom: 12),
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 16),
      decoration: BoxDecoration(
        color: Colors.white.withOpacity(0.04),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: isOnline
              ? Colors.greenAccent.withOpacity(0.15)
              : Colors.white.withOpacity(0.05),
        ),
      ),
      child: Row(
        children: [
          // Status indicator dot
          Container(
            width: 10,
            height: 10,
            decoration: BoxDecoration(
              color: statusColor,
              shape: BoxShape.circle,
              boxShadow: [
                BoxShadow(
                  color: statusColor.withOpacity(0.5),
                  blurRadius: 8,
                  spreadRadius: 2,
                )
              ],
            ),
          ),
          const SizedBox(width: 20),

          // Host details
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  host["name"] ?? "Unnamed Host",
                  style: const TextStyle(
                    color: Colors.white,
                    fontWeight: FontWeight.bold,
                    fontSize: 16,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  "IP: ${host["ip_address"] ?? "Unknown"} • $status",
                  style: TextStyle(
                    color: Colors.grey.shade400,
                    fontSize: 12,
                  ),
                ),
              ],
            ),
          ),

          // Actions
          Row(
            children: [
              if (isOnline)
                ElevatedButton(
                  onPressed: () => _showAppSelectionDialog(host),
                  style: ElevatedButton.styleFrom(
                    backgroundColor: Colors.blueAccent.shade700,
                    foregroundColor: Colors.white,
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(10),
                    ),
                  ),
                  child: const Text("Connect"),
                ),
              const SizedBox(width: 8),
              IconButton(
                onPressed: () => _unpairHost(host["id"]),
                icon: const Icon(Icons.delete_outline, color: Colors.grey),
                tooltip: "Remove Host",
              ),
            ],
          )
        ],
      ),
    );
  }

  Widget _buildAgentCard() {
    return Container(
      padding: const EdgeInsets.all(20),
      decoration: BoxDecoration(
        color: Colors.white.withOpacity(0.03),
        borderRadius: BorderRadius.circular(24),
        border: Border.all(color: Colors.white.withOpacity(0.07)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.settings_remote, color: Colors.blueAccent, size: 24),
              const SizedBox(width: 10),
              const Text(
                "Host Mode (Agent)",
                style: TextStyle(
                  color: Colors.white,
                  fontWeight: FontWeight.bold,
                  fontSize: 16,
                ),
              ),
              const Spacer(),
              Switch(
                value: _allowRemoteControl,
                onChanged: _toggleAgent,
                activeColor: Colors.blueAccent,
              ),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            _allowRemoteControl
                ? "Allow Remote Control: ACTIVE. This machine is discoverable."
                : "Allow Remote Control: DISABLED. This machine cannot be streamed.",
            style: TextStyle(
              color: _allowRemoteControl ? Colors.greenAccent : Colors.grey,
              fontSize: 12,
            ),
          ),
          if (_allowRemoteControl || _agentLogs.isNotEmpty) ...[
            const SizedBox(height: 16),
            Container(
              height: 100,
              width: double.infinity,
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: Colors.black45,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: Colors.white.withOpacity(0.05)),
              ),
              child: SingleChildScrollView(
                reverse: true,
                child: Text(
                  _agentLogs.isEmpty ? "Starting logs..." : _agentLogs.join('\n'),
                  style: const TextStyle(
                    color: Colors.grey,
                    fontFamily: 'monospace',
                    fontSize: 10,
                  ),
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildPairingCard() {
    return Container(
      padding: const EdgeInsets.all(20),
      decoration: BoxDecoration(
        color: Colors.white.withOpacity(0.03),
        borderRadius: BorderRadius.circular(24),
        border: Border.all(color: Colors.white.withOpacity(0.07)),
      ),
      child: Form(
        key: _pairFormKey,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                "Pair New Host",
                style: TextStyle(
                  color: Colors.white,
                  fontWeight: FontWeight.bold,
                  fontSize: 16,
                ),
              ),
              const SizedBox(height: 12),
              _buildTextField(
                controller: _nameController,
                label: "Host Alias (e.g. Work PC)",
                validator: (val) => val == null || val.isEmpty ? "Alias required" : null,
              ),
              const SizedBox(height: 10),
              _buildTextField(
                controller: _ipController,
                label: "Host IP Address",
                validator: (val) => val == null || val.isEmpty ? "IP address required" : null,
              ),
              const SizedBox(height: 10),
              _buildTextField(
                controller: _sunshineUserController,
                label: "Sunshine Web Username",
                validator: (val) => val == null || val.isEmpty ? "Sunshine username required" : null,
              ),
              const SizedBox(height: 10),
              _buildTextField(
                controller: _sunshinePassController,
                label: "Sunshine Web Password",
                obscure: true,
                validator: (val) => val == null || val.isEmpty ? "Sunshine password required" : null,
              ),
              const SizedBox(height: 16),
              if (_pairingError != null)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12.0),
                  child: Text(
                    _pairingError!,
                    style: const TextStyle(color: Colors.redAccent, fontSize: 12),
                  ),
                ),
              if (_pairingSuccess)
                const Padding(
                  padding: const EdgeInsets.only(bottom: 12.0),
                  child: Text(
                    "Host paired successfully!",
                    style: TextStyle(color: Colors.greenAccent, fontSize: 12),
                  ),
                ),
              SizedBox(
                width: double.infinity,
                child: ElevatedButton(
                  onPressed: _isPairing ? null : _pairHost,
                  style: ElevatedButton.styleFrom(
                    backgroundColor: Colors.blueAccent.shade700,
                    foregroundColor: Colors.white,
                    padding: const EdgeInsets.symmetric(vertical: 14),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(12),
                    ),
                  ),
                  child: _isPairing
                      ? const SizedBox(
                          height: 18,
                          width: 18,
                          child: CircularProgressIndicator(strokeWidth: 2, color: Colors.white),
                        )
                      : const Text("Execute Pairing"),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildTextField({
    required TextEditingController controller,
    required String label,
    bool obscure = false,
    String? Function(String?)? validator,
  }) {
    return TextFormField(
      controller: controller,
      obscureText: obscure,
      validator: validator,
      style: const TextStyle(color: Colors.white, fontSize: 13),
      decoration: InputDecoration(
        labelText: label,
        labelStyle: TextStyle(color: Colors.grey.shade500, fontSize: 12),
        filled: true,
        fillColor: Colors.black.withOpacity(0.3),
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(12),
          borderSide: BorderSide(color: Colors.white.withOpacity(0.08)),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(12),
          borderSide: const BorderSide(color: Colors.blueAccent, width: 1.5),
        ),
        errorBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(12),
          borderSide: const BorderSide(color: Colors.redAccent, width: 1),
        ),
        focusedErrorBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(12),
          borderSide: const BorderSide(color: Colors.redAccent, width: 1.5),
        ),
      ),
    );
  }

  void _showAppSelectionDialog(Map<String, dynamic> host) {
    _queryAppsForHost(host);

    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (context) {
        return StatefulBuilder(
          builder: (context, setModalState) {
            _modalStateSetter = setModalState;
            final appLoading = _isLoadingApps;
            return Container(
              margin: const EdgeInsets.only(top: 80),
              decoration: BoxDecoration(
                color: const Color(0xFF13151A),
                borderRadius: const BorderRadius.only(
                  topLeft: Radius.circular(28),
                  topRight: Radius.circular(28),
                ),
                border: Border.all(color: Colors.white.withOpacity(0.08)),
              ),
              padding: const EdgeInsets.all(24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  // Dialog Header
                  Row(
                    children: [
                      Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            "Launch Application",
                            style: const TextStyle(
                              color: Colors.white,
                              fontWeight: FontWeight.bold,
                              fontSize: 20,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            "Querying ${host["name"]}",
                            style: TextStyle(color: Colors.grey.shade400, fontSize: 13),
                          ),
                        ],
                      ),
                      const Spacer(),
                      IconButton(
                        onPressed: () {
                          _appQuerySignaling?.dispose();
                          _appQuerySignaling = null;
                          Navigator.of(context).pop();
                        },
                        icon: const Icon(Icons.close, color: Colors.grey),
                      )
                    ],
                  ),
                  const Divider(color: Colors.white10, height: 24),

                  // Content Layout: Two sections (Settings Left, App List Right)
                  Flexible(
                    child: SingleChildScrollView(
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                      // Stream settings config
                      Expanded(
                        flex: 2,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            const Text(
                              "Quality Settings",
                              style: TextStyle(
                                color: Colors.blueAccent,
                                fontWeight: FontWeight.bold,
                                fontSize: 14,
                              ),
                            ),
                            const SizedBox(height: 16),
                            _buildSettingsDropdown(
                              label: "Resolution",
                              value: _streamRes,
                              items: ["1080p", "720p", "540p"],
                              onChanged: (val) {
                                if (val != null) {
                                  setState(() => _streamRes = val);
                                  setModalState(() {});
                                }
                              },
                            ),
                            const SizedBox(height: 12),
                            _buildSettingsDropdown(
                              label: "Frame Rate",
                              value: _streamFps.toString(),
                              items: ["240", "144", "120", "90", "60", "30"],
                              onChanged: (val) {
                                if (val != null) {
                                  setState(() => _streamFps = int.parse(val));
                                  setModalState(() {});
                                }
                              },
                            ),
                            const SizedBox(height: 12),
                            _buildSettingsDropdown(
                              label: "Bitrate Limit",
                              value: "${_streamBitrate ~/ 1000} Mbps",
                              items: ["150 Mbps", "100 Mbps", "80 Mbps", "50 Mbps", "30 Mbps", "20 Mbps", "15 Mbps", "10 Mbps", "8 Mbps", "5 Mbps", "3 Mbps"],
                              onChanged: (val) {
                                if (val != null) {
                                  final m = int.parse(val.split(' ')[0]);
                                  setState(() => _streamBitrate = m * 1000);
                                  setModalState(() {});
                                }
                              },
                            ),
                            const SizedBox(height: 12),
                            Builder(
                              builder: (context) {
                                final hostCodecSupport = (host["server_codec_mode_support"] as num?)?.toInt() ?? 0;
                                final hostH265Supported = hostCodecSupport == 0 || (hostCodecSupport & 1573632) != 0;
                                final hostAv1Supported = hostCodecSupport != 0 && (hostCodecSupport & 6488064) != 0;

                                final List<String> availableCodecs = ["H264"];
                                final clientH265 = _isH265SupportedByClient();
                                if (clientH265 && hostH265Supported) {
                                  availableCodecs.add("H265");
                                }
                                final clientAv1 = _isAv1SupportedByClient();
                                if (clientAv1 && hostAv1Supported) {
                                  availableCodecs.add("AV1");
                                }

                                if (!availableCodecs.contains(_streamCodec.toUpperCase())) {
                                  _streamCodec = "h264";
                                }

                                return Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    _buildSettingsDropdown(
                                      label: "Video Codec",
                                      value: _streamCodec.toUpperCase(),
                                      items: availableCodecs,
                                      onChanged: (val) {
                                        if (val != null) {
                                          setState(() => _streamCodec = val.toLowerCase());
                                          setModalState(() {});
                                        }
                                      },
                                    ),
                                    if (!clientH265) ...[
                                      const SizedBox(height: 6),
                                      Text(
                                        "* H.265 is unsupported on Linux/Windows desktop clients.",
                                        style: TextStyle(color: Colors.grey.shade500, fontSize: 10, fontStyle: FontStyle.italic),
                                      ),
                                    ] else if (!hostH265Supported) ...[
                                      const SizedBox(height: 6),
                                      Text(
                                        "* H.265 is unsupported by the host agent.",
                                        style: TextStyle(color: Colors.grey.shade500, fontSize: 10, fontStyle: FontStyle.italic),
                                      ),
                                    ],
                                  ],
                                );
                              }
                            ),
                          ],
                        ),
                      ),
                      const SizedBox(width: 32),

                      // Apps Directory list
                      Expanded(
                        flex: 3,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            const Text(
                              "Select Application",
                              style: TextStyle(
                                color: Colors.white,
                                fontWeight: FontWeight.bold,
                                fontSize: 14,
                              ),
                            ),
                            const SizedBox(height: 16),
                            SizedBox(
                              height: 300,
                              child: appLoading
                                  ? const Center(child: CircularProgressIndicator())
                                  : _hostApps.isEmpty
                                      ? Center(
                                          child: Text(
                                            "No apps discovered.",
                                            style: TextStyle(color: Colors.grey.shade500),
                                          ),
                                        )
                                      : ListView.builder(
                                          itemCount: _hostApps.length,
                                          itemBuilder: (context, index) {
                                            final app = _hostApps[index];
                                            return _buildAppItem(app);
                                          },
                                        ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        );
          },
        );
      },
    ).then((_) {
      // Safe guard cleanup of signaling
      _appQuerySignaling?.dispose();
      _appQuerySignaling = null;
      _modalStateSetter = null;
    });
  }

  Widget _buildSettingsDropdown({
    required String label,
    required String value,
    required List<String> items,
    required ValueChanged<String?> onChanged,
  }) {
    final List<String> safeItems = List.from(items);
    if (!safeItems.contains(value)) {
      safeItems.add(value);
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: TextStyle(color: Colors.grey.shade400, fontSize: 12),
        ),
        const SizedBox(height: 6),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color: Colors.black26,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: Colors.white.withOpacity(0.08)),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<String>(
              value: value,
              items: safeItems
                  .map((item) => DropdownMenuItem(
                        value: item,
                        child: Text(item, style: const TextStyle(color: Colors.white, fontSize: 13)),
                      ))
                  .toList(),
              onChanged: onChanged,
              dropdownColor: const Color(0xFF1C1E24),
              icon: const Icon(Icons.keyboard_arrow_down, color: Colors.grey, size: 18),
              isExpanded: true,
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildAppItem(dynamic app) {
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Colors.white.withOpacity(0.03),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: Colors.white.withOpacity(0.05)),
      ),
      child: Row(
        children: [
          const Icon(Icons.rocket_launch, color: Colors.blueAccent, size: 20),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              app["title"] ?? "Unnamed App",
              style: const TextStyle(
                color: Colors.white,
                fontWeight: FontWeight.bold,
                fontSize: 14,
              ),
            ),
          ),
          ElevatedButton(
            onPressed: () => _launchApp(app),
            style: ElevatedButton.styleFrom(
              backgroundColor: Colors.greenAccent.shade700,
              foregroundColor: Colors.white,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(8),
              ),
            ),
            child: const Text("Launch", style: TextStyle(fontSize: 12)),
          ),
        ],
      ),
    );
  }

  bool _isH265SupportedByClient() {
    if (kIsWeb) return false;
    if (Platform.isLinux || Platform.isWindows) return false;
    return true;
  }

  bool _isAv1SupportedByClient() {
    return true;
  }


}
