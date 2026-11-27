#include "qt_app.h"
#include <QAction>
#include <QApplication>
#include <QByteArray>
#include <QCursor>
#include <QDesktopServices>
#include <QIcon>
#include <QLocale>
#include <QMenu>
#include <QSystemTrayIcon>
#include <QTimer>
#include <QUrl>
#include <cstddef>

// tray.rs
extern "C" {
bool lunaris_helper_connected();
std::size_t lunaris_helper_web_url(unsigned char *buf, std::size_t len);
void lunaris_helper_restart();
}

namespace {
// QApplication keeps references to argc/argv for its whole lifetime.
int g_argc = 1;
char g_arg0[] = "lunaris-approval-helper";
char *g_argv[] = {g_arg0, nullptr};
QApplication *g_app = nullptr;
QSystemTrayIcon *g_tray = nullptr;

// The texts of client-qml/qml/I18n.qml; Vietnamese when the system is.
QString text(const char *en, const char *vi) {
    return QString::fromUtf8(QLocale::system().name().startsWith(QLatin1String("vi")) ? vi : en);
}

QString web_url() {
    QByteArray buf(2048, '\0');
    std::size_t n = lunaris_helper_web_url(reinterpret_cast<unsigned char *>(buf.data()), buf.size());
    if (n == 0 || n > static_cast<std::size_t>(buf.size()))
        return QString();
    return QString::fromUtf8(buf.constData(), static_cast<int>(n));
}
} // namespace

void create_widgets_application() {
    if (!g_app) {
        g_app = new QApplication(g_argc, g_argv);
        // Only a tray and a window that hides: keep running.
        QApplication::setQuitOnLastWindowClosed(false);
    }
}

void create_tray() {
    if (g_tray || !g_app)
        return;
    // A QMenu and QSystemTrayIcon of Qt Widgets: the Qt.labs.platform menu
    // showed nothing on right-click (Windows, and Linux trays).
    auto *menu = new QMenu();
    QAction *status = menu->addAction(text("You are asked before anyone connects without the password",
                                           "Bạn sẽ được hỏi trước khi ai đó kết nối mà không có mật khẩu"));
    status->setEnabled(false);
    menu->addSeparator();
    QAction *open = menu->addAction(text("Open the Lunaris web page", "Mở trang web Lunaris"));
    QAction *restart = menu->addAction(text("Restart the agent", "Khởi động lại agent"));
    menu->addSeparator();
    QAction *quit = menu->addAction(text("Exit", "Thoát"));

    QObject::connect(open, &QAction::triggered, [] {
        const QString url = web_url();
        if (!url.isEmpty())
            QDesktopServices::openUrl(QUrl(url));
    });
    QObject::connect(restart, &QAction::triggered, [] { lunaris_helper_restart(); });
    QObject::connect(quit, &QAction::triggered, [] { QApplication::quit(); });

    g_tray = new QSystemTrayIcon(QIcon(QStringLiteral(":/icons/icon.png")));
    g_tray->setToolTip(QStringLiteral("Lunaris Agent"));
    g_tray->setContextMenu(menu);
    // A left click shows the same menu (Windows gives it only the right).
    QObject::connect(g_tray, &QSystemTrayIcon::activated, [menu](QSystemTrayIcon::ActivationReason reason) {
        if (reason == QSystemTrayIcon::Trigger)
            menu->popup(QCursor::pos());
    });

    // Shown while an agent lets this helper answer; what it offers follows.
    auto refresh = [open, restart] {
        const bool connected = lunaris_helper_connected();
        open->setEnabled(connected && !web_url().isEmpty());
        restart->setEnabled(connected);
        if (g_tray->isVisible() != connected)
            g_tray->setVisible(connected);
    };
    auto *timer = new QTimer(g_tray);
    QObject::connect(timer, &QTimer::timeout, refresh);
    timer->start(1000);
    // The menu is up to date when it opens, not up to a second late.
    QObject::connect(menu, &QMenu::aboutToShow, refresh);
    refresh();
}

void tray_show_message(const QString &title, const QString &message, int timeout_ms) {
    if (g_tray && g_tray->isVisible() && QSystemTrayIcon::supportsMessages())
        g_tray->showMessage(title, message, QSystemTrayIcon::Information, timeout_ms);
}

int exec_widgets_application() {
    return QApplication::exec();
}
