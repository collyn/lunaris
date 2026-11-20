#include "qt_app.h"
#include <QApplication>

namespace {
// QApplication keeps references to argc/argv for its whole lifetime.
int g_argc = 1;
char g_arg0[] = "lunaris-approval-helper";
char *g_argv[] = {g_arg0, nullptr};
QApplication *g_app = nullptr;
}

void create_widgets_application() {
    if (!g_app) {
        g_app = new QApplication(g_argc, g_argv);
        // Only a tray and a window that hides: keep running.
        QApplication::setQuitOnLastWindowClosed(false);
    }
}

int exec_widgets_application() {
    return QApplication::exec();
}
