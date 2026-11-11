#ifndef AGENT_GUI_H
#define AGENT_GUI_H

#include <QString>

// Opens a URL in the platform's default handler (browser / protocol handler).
void open_url_helper(const QString &url);

// Opens a native file dialog for importing a JSON agent config.
// Returns the absolute file path, or an empty string if cancelled.
QString pick_import_file();

// Creates the process-wide QApplication. A QWidgets application (not just
// QGuiApplication) is required because Qt.labs.platform falls back to
// widget-based tray menus on Linux and pick_import_file() uses QFileDialog;
// without it Qt aborts with "Cannot create a QWidget without QApplication".
void create_widgets_application();

// Runs the Qt event loop of the application created above.
int exec_widgets_application();

#endif
