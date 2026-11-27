#ifndef LUNARIS_APPROVAL_QT_APP_H
#define LUNARIS_APPROVAL_QT_APP_H

#include <QString>

// Creates the process-wide QApplication: a QWidgets one, for the tray's
// QSystemTrayIcon and its QMenu.
void create_widgets_application();

// The tray icon and its menu (shown while an agent is connected): the
// agent's web page, restarting the agent, and leaving this helper.
void create_tray();

// A notification from the tray icon, where the system shows them.
void tray_show_message(const QString &title, const QString &text, int timeout_ms);

// Runs the Qt event loop of the application created above.
int exec_widgets_application();

#endif
