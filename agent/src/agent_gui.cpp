#include "agent_gui.h"
#include <QDesktopServices>
#include <QUrl>
#include <QFileDialog>

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
