#ifndef LUNARIS_APPROVAL_QT_APP_H
#define LUNARIS_APPROVAL_QT_APP_H

// Creates the process-wide QApplication: a QWidgets one, because
// Qt.labs.platform falls back to widget-based tray menus on Linux.
void create_widgets_application();

// Runs the Qt event loop of the application created above.
int exec_widgets_application();

#endif
