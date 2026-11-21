// Apply the saved theme before first paint (no flash). "system" = no attribute.
// A file, not an inline <script>: the server's Content-Security-Policy
// allows scripts from 'self' only.
try {
  var t = localStorage.getItem('lunaris_theme');
  if (t === 'light' || t === 'dark') document.documentElement.setAttribute('data-theme', t);
} catch (e) {}
