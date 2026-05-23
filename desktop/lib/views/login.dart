import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:shared_preferences/shared_preferences.dart';

class LoginView extends StatefulWidget {
  final Function(String serverHost, String token, String username) onLoginSuccess;

  const LoginView({super.key, required this.onLoginSuccess});

  @override
  State<LoginView> createState() => _LoginViewState();
}

class _LoginViewState extends State<LoginView> {
  final _formKey = GlobalKey<FormState>();
  final _serverController = TextEditingController(text: "localhost:8080");
  final _usernameController = TextEditingController();
  final _passwordController = TextEditingController();
  final _confirmPasswordController = TextEditingController();

  bool _isRegister = false;
  bool _isLoading = false;
  String? _errorMessage;

  Future<void> _submit() async {
    if (!_formKey.currentState!.validate()) return;

    setState(() {
      _isLoading = true;
      _errorMessage = null;
    });

    final serverHost = _serverController.text.trim();
    final username = _usernameController.text.trim();
    final password = _passwordController.text;

    final protocol = serverHost.startsWith("localhost") ? "http" : "http"; // default http, wss handles secure ws
    final endpoint = _isRegister ? "/api/auth/register" : "/api/auth/login";
    final url = Uri.parse("$protocol://$serverHost$endpoint");

    try {
      final response = await http.post(
        url,
        headers: {"Content-Type": "application/json"},
        body: jsonEncode({
          "username": username,
          "password": password,
        }),
      );

      final data = jsonDecode(response.body);

      if (response.statusCode == 200) {
        final token = data["token"];
        final resUsername = data["username"] ?? username;

        // Persist token & details
        final prefs = await SharedPreferences.getInstance();
        await prefs.setString("lunaris_token", token);
        await prefs.setString("lunaris_server_host", serverHost);
        await prefs.setString("lunaris_username", resUsername);

        widget.onLoginSuccess(serverHost, token, resUsername);
      } else {
        setState(() {
          _errorMessage = data["error"] ?? "Authentication failed.";
        });
      }
    } catch (e) {
      setState(() {
        _errorMessage = "Connection to authentication server failed: $e";
      });
    } finally {
      setState(() {
        _isLoading = false;
      });
    }
  }

  @override
  void dispose() {
    _serverController.dispose();
    _usernameController.dispose();
    _passwordController.dispose();
    _confirmPasswordController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: const Color(0xFF0D0E12),
      body: Container(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: Alignment.center,
            radius: 1.2,
            colors: [
              Colors.deepPurple.shade900.withOpacity(0.15),
              const Color(0xFF0D0E12),
            ],
          ),
        ),
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(24.0),
            child: Form(
              key: _formKey,
              child: Container(
                constraints: const BoxConstraints(maxWidth: 450),
                padding: const EdgeInsets.all(32.0),
                decoration: BoxDecoration(
                  color: Colors.white.withOpacity(0.04),
                  borderRadius: BorderRadius.circular(24),
                  border: Border.all(
                    color: Colors.white.withOpacity(0.08),
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
                    // Logo / Icon
                    const Icon(
                      Icons.blur_on_rounded,
                      color: Colors.blueAccent,
                      size: 64,
                    ),
                    const SizedBox(height: 16),
                    const Text(
                      "LUNARIS",
                      style: TextStyle(
                        color: Colors.white,
                        fontWeight: FontWeight.w900,
                        fontSize: 28,
                        letterSpacing: 4,
                      ),
                    ),
                    Text(
                      _isRegister ? "Create a new host account" : "Sign in to connect & stream",
                      style: TextStyle(
                        color: Colors.grey.shade500,
                        fontSize: 14,
                      ),
                    ),
                    const SizedBox(height: 32),

                    // Inputs
                    _buildTextField(
                      controller: _serverController,
                      label: "Signaling Server Host",
                      icon: Icons.dns_outlined,
                      validator: (val) =>
                          val == null || val.isEmpty ? "Server host is required" : null,
                    ),
                    const SizedBox(height: 16),
                    _buildTextField(
                      controller: _usernameController,
                      label: "Username",
                      icon: Icons.person_outline,
                      validator: (val) =>
                          val == null || val.isEmpty ? "Username is required" : null,
                    ),
                    const SizedBox(height: 16),
                    _buildTextField(
                      controller: _passwordController,
                      label: "Password",
                      icon: Icons.lock_outline,
                      obscure: true,
                      validator: (val) => val == null || val.length < 6
                          ? "Password must be at least 6 characters"
                          : null,
                    ),
                    if (_isRegister) ...[
                      const SizedBox(height: 16),
                      _buildTextField(
                        controller: _confirmPasswordController,
                        label: "Confirm Password",
                        icon: Icons.lock_outline,
                        obscure: true,
                        validator: (val) => val != _passwordController.text
                            ? "Passwords do not match"
                            : null,
                      ),
                    ],
                    const SizedBox(height: 24),

                    // Error Message
                    if (_errorMessage != null) ...[
                      Text(
                        _errorMessage!,
                        style: const TextStyle(color: Colors.redAccent, fontSize: 13),
                        textAlign: TextAlign.center,
                      ),
                      const SizedBox(height: 16),
                    ],

                    // Action Button
                    SizedBox(
                      width: double.infinity,
                      child: ElevatedButton(
                        onPressed: _isLoading ? null : _submit,
                        style: ElevatedButton.styleFrom(
                          backgroundColor: Colors.blueAccent.shade700,
                          foregroundColor: Colors.white,
                          padding: const EdgeInsets.symmetric(vertical: 16),
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(14),
                          ),
                          elevation: 5,
                        ),
                        child: _isLoading
                            ? const SizedBox(
                                height: 20,
                                width: 20,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                  color: Colors.white,
                                ),
                              )
                            : Text(
                                _isRegister ? "REGISTER" : "LOGIN",
                                style: const TextStyle(
                                  fontWeight: FontWeight.bold,
                                  letterSpacing: 1.5,
                                ),
                              ),
                      ),
                    ),
                    const SizedBox(height: 16),

                    // Toggle Link
                    TextButton(
                      onPressed: () {
                        setState(() {
                          _isRegister = !_isRegister;
                          _errorMessage = null;
                        });
                      },
                      style: TextButton.styleFrom(
                        foregroundColor: Colors.grey.shade400,
                      ),
                      child: Text(
                        _isRegister
                            ? "Already have an account? Sign In"
                            : "Don't have an account? Register Now",
                        style: const TextStyle(fontSize: 13),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildTextField({
    required TextEditingController controller,
    required String label,
    required IconData icon,
    bool obscure = false,
    String? Function(String?)? validator,
  }) {
    return TextFormField(
      controller: controller,
      obscureText: obscure,
      validator: validator,
      style: const TextStyle(color: Colors.white, fontSize: 14),
      decoration: InputDecoration(
        labelText: label,
        labelStyle: TextStyle(color: Colors.grey.shade500, fontSize: 13),
        prefixIcon: Icon(icon, color: Colors.grey.shade500, size: 20),
        filled: true,
        fillColor: Colors.black.withOpacity(0.3),
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(14),
          borderSide: BorderSide(color: Colors.white.withOpacity(0.06)),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(14),
          borderSide: const BorderSide(color: Colors.blueAccent, width: 1.5),
        ),
        errorBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(14),
          borderSide: const BorderSide(color: Colors.redAccent, width: 1),
        ),
        focusedErrorBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(14),
          borderSide: const BorderSide(color: Colors.redAccent, width: 1.5),
        ),
      ),
    );
  }
}
