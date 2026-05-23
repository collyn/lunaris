import 'dart:io';
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'views/login.dart';
import 'views/dashboard.dart';
import 'views/stream_player.dart';

Future<void> registerProtocol() async {
  try {
    final exePath = Platform.resolvedExecutable;
    if (Platform.isLinux) {
      final home = Platform.environment['HOME'];
      if (home == null) return;
      final destDir = Directory('$home/.local/share/applications');
      if (!await destDir.exists()) {
        await destDir.create(recursive: true);
      }
      final destFile = File('${destDir.path}/lunaris-client.desktop');
      final content = '''
[Desktop Entry]
Type=Application
Name=Lunaris Client
Exec='$exePath' %u
Terminal=false
MimeType=x-scheme-handler/lunaris;
Categories=Network;
''';
      await destFile.writeAsString(content);
      
      // Register mimetype handler via xdg-mime
      await Process.run('xdg-mime', ['default', 'lunaris-client.desktop', 'x-scheme-handler/lunaris']);
      // Update desktop database
      await Process.run('update-desktop-database', [destDir.path]);
      debugPrint('Registered Linux desktop entry for lunaris://');
    } else if (Platform.isWindows) {
      // Register Windows registry protocol (using reg.exe commands)
      await Process.run('reg', [
        'add',
        'HKCU\\Software\\Classes\\lunaris',
        '/v',
        'URL Protocol',
        '/t',
        'REG_SZ',
        '/d',
        '',
        '/f'
      ]);
      await Process.run('reg', [
        'add',
        'HKCU\\Software\\Classes\\lunaris\\shell\\open\\command',
        '/ve',
        '/t',
        'REG_SZ',
        '/d',
        '"$exePath" "%1"',
        '/f'
      ]);
      debugPrint('Registered Windows registry for lunaris://');
    }
  } catch (e) {
    debugPrint('Failed to register protocol: $e');
  }
}

void main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  
  await registerProtocol();

  String? initialToken;
  String? initialServerHost;
  String? initialUsername;
  
  // Parsed deep link parameters if available
  String? deepLinkHostId;
  String? deepLinkResolution;
  int? deepLinkFps;
  int? deepLinkBitrate;
  String? deepLinkCodec;

  if (args.isNotEmpty && args.first.startsWith('lunaris://')) {
    try {
      final uri = Uri.parse(args.first);
      if (uri.scheme == 'lunaris' && uri.host == 'connect') {
        deepLinkHostId = uri.queryParameters['host_id'];
        final serverUrl = uri.queryParameters['server']; // e.g. ws://localhost:8080 or http://localhost:8080
        initialToken = uri.queryParameters['token'];
        
        // Extract host/port from serverUrl
        if (serverUrl != null) {
          final cleanedUrl = serverUrl
              .replaceAll('ws://', '')
              .replaceAll('wss://', '')
              .replaceAll('http://', '')
              .replaceAll('https://', '');
          initialServerHost = cleanedUrl;
        }
        
        initialUsername = "Remote User"; // Placeholder
        deepLinkResolution = uri.queryParameters['res'] ?? '1080p';
        deepLinkFps = int.tryParse(uri.queryParameters['fps'] ?? '') ?? 60;
        deepLinkBitrate = int.tryParse(uri.queryParameters['bitrate'] ?? '') ?? 8000;
        deepLinkCodec = uri.queryParameters['codec'] ?? 'h264';

        // Persist session to SharedPreferences so it acts as logged in
        final prefs = await SharedPreferences.getInstance();
        if (initialToken != null) await prefs.setString("lunaris_token", initialToken);
        if (initialServerHost != null) await prefs.setString("lunaris_server_host", initialServerHost);
        await prefs.setString("lunaris_username", "Remote User");
      }
    } catch (e) {
      debugPrint("Error parsing deep link: $e");
    }
  } else {
    // Normal launch: load from preferences
    final prefs = await SharedPreferences.getInstance();
    initialToken = prefs.getString("lunaris_token");
    initialServerHost = prefs.getString("lunaris_server_host");
    initialUsername = prefs.getString("lunaris_username");
  }

  runApp(MyApp(
    initialToken: initialToken,
    initialServerHost: initialServerHost,
    initialUsername: initialUsername,
    deepLinkHostId: deepLinkHostId,
    deepLinkResolution: deepLinkResolution,
    deepLinkFps: deepLinkFps,
    deepLinkBitrate: deepLinkBitrate,
    deepLinkCodec: deepLinkCodec,
  ));
}

class MyApp extends StatefulWidget {
  final String? initialToken;
  final String? initialServerHost;
  final String? initialUsername;
  
  // Deep link specific params
  final String? deepLinkHostId;
  final String? deepLinkResolution;
  final int? deepLinkFps;
  final int? deepLinkBitrate;
  final String? deepLinkCodec;

  const MyApp({
    super.key,
    this.initialToken,
    this.initialServerHost,
    this.initialUsername,
    this.deepLinkHostId,
    this.deepLinkResolution,
    this.deepLinkFps,
    this.deepLinkBitrate,
    this.deepLinkCodec,
  });

  @override
  State<MyApp> createState() => _MyAppState();
}

class _MyAppState extends State<MyApp> {
  String? _token;
  String? _serverHost;
  String? _username;
  bool _showDeepLink = false;

  @override
  void initState() {
    super.initState();
    _token = widget.initialToken;
    _serverHost = widget.initialServerHost;
    _username = widget.initialUsername;
    _showDeepLink = widget.deepLinkHostId != null;
  }

  void _onLoginSuccess(String serverHost, String token, String username) {
    setState(() {
      _serverHost = serverHost;
      _token = token;
      _username = username;
    });
  }

  Future<void> _onLogout() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.remove("lunaris_token");
    await prefs.remove("lunaris_server_host");
    await prefs.remove("lunaris_username");
    
    setState(() {
      _token = null;
      _username = null;
      _serverHost = null;
      _showDeepLink = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final isLoggedIn = _token != null && _serverHost != null && _username != null;

    Widget homeWidget;
    if (_showDeepLink && widget.deepLinkHostId != null && _token != null && _serverHost != null) {
      homeWidget = StreamPlayerView(
        serverHost: _serverHost!,
        token: _token!,
        hostId: widget.deepLinkHostId!,
        hostName: "Remote Host",
        appId: 1, // Default to Desktop
        appName: "Desktop",
        resolution: widget.deepLinkResolution ?? "1080p",
        fps: widget.deepLinkFps ?? 60,
        bitrate: widget.deepLinkBitrate ?? 8000,
        codec: widget.deepLinkCodec ?? "h264",
        onBack: () {
          setState(() {
            _showDeepLink = false;
          });
        },
      );
    } else if (isLoggedIn) {
      homeWidget = DashboardView(
        serverHost: _serverHost!,
        token: _token!,
        username: _username!,
        onLogout: _onLogout,
      );
    } else {
      homeWidget = LoginView(
        onLoginSuccess: _onLoginSuccess,
      );
    }

    return MaterialApp(
      title: 'Lunaris Desktop Client',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        useMaterial3: true,
        primaryColor: Colors.blueAccent,
        colorScheme: ColorScheme.dark(
          primary: Colors.blueAccent,
          secondary: Colors.deepPurpleAccent,
          background: const Color(0xFF0D0E12),
          surface: Colors.white.withOpacity(0.04),
        ),
        textTheme: const TextTheme(
          bodyLarge: TextStyle(color: Colors.white70),
          bodyMedium: TextStyle(color: Colors.white60),
        ),
      ),
      home: homeWidget,
    );
  }
}
