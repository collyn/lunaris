#include "agent_gui.h"
#include <QDesktopServices>
#include <QUrl>
#include <QFileDialog>
#include <QApplication>

namespace {
// QApplication keeps references to argc/argv for its whole lifetime.
int g_argc = 1;
char g_arg0[] = "agent";
char *g_argv[] = {g_arg0, nullptr};
QApplication *g_app = nullptr;
}

void create_widgets_application() {
    if (!g_app) {
        g_app = new QApplication(g_argc, g_argv);
    }
}

int exec_widgets_application() {
    return QApplication::exec();
}

void open_url_helper(const QString &url) {
    QDesktopServices::openUrl(QUrl(url));
}

QString pick_import_file() {
    return QFileDialog::getOpenFileName(
        nullptr,
        "Select agent_config.json",
        QString(),
        "JSON Config (*.json)"
    );
}
