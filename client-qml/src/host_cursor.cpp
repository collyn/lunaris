#include "host_cursor.h"

#include <QByteArray>
#include <QCursor>
#include <QPixmap>
#include <QtQml>

HostCursor::HostCursor(QObject *parent) : QObject(parent) {}

void HostCursor::registerTypes() {
    qmlRegisterType<HostCursor>("com.lunaris.client.gpu", 1, 0, "HostCursor");
}

#define HOST_CURSOR_SETTER(name, member, type)  \
    void HostCursor::name(type value) {          \
        if (member == value) return;             \
        member = value;                          \
        emit changed();                          \
        scheduleApply();                         \
    }

HOST_CURSOR_SETTER(setActive, m_active, bool)
HOST_CURSOR_SETTER(setHidden, m_hidden, bool)
HOST_CURSOR_SETTER(setSource, m_source, const QString &)
HOST_CURSOR_SETTER(setCursorWidth, m_width, int)
HOST_CURSOR_SETTER(setCursorHeight, m_height, int)
HOST_CURSOR_SETTER(setHotspotX, m_hotX, int)
HOST_CURSOR_SETTER(setHotspotY, m_hotY, int)

void HostCursor::setTarget(QQuickItem *target) {
    if (m_target == target) return;
    m_target = target;
    emit changed();
    scheduleApply();
}

// Coalesce property changes (kind, image, size and hotspot usually change
// together) and run after QML bindings settled, so the MouseArea's own
// cursorShape binding never overwrites the pixmap cursor afterwards.
void HostCursor::scheduleApply() {
    if (m_applyQueued) return;
    m_applyQueued = true;
    QMetaObject::invokeMethod(this, &HostCursor::apply, Qt::QueuedConnection);
}

static QPixmap loadCursorPixmap(const QString &source) {
    QPixmap pixmap;
    if (source.startsWith(QLatin1String("data:"))) {
        const int comma = source.indexOf(QLatin1Char(','));
        if (comma > 0 && source.left(comma).endsWith(QLatin1String(";base64"))) {
            pixmap.loadFromData(QByteArray::fromBase64(source.mid(comma + 1).toLatin1()));
        }
    } else if (source.startsWith(QLatin1String("qrc:"))) {
        pixmap.load(source.mid(3));  // "qrc:/x" -> ":/x"
    } else if (!source.isEmpty()) {
        pixmap.load(source);
    }
    return pixmap;
}

void HostCursor::apply() {
    m_applyQueued = false;
    if (!m_active || !m_target) return;

    if (m_hidden || m_source.isEmpty()) {
        m_target->setCursor(QCursor(Qt::BlankCursor));
        return;
    }
    if (m_pixmap.isNull() || m_loadedSource != m_source) {
        QPixmap loaded = loadCursorPixmap(m_source);
        if (loaded.isNull()) {
            m_target->setCursor(QCursor(Qt::ArrowCursor));
            return;
        }
        m_pixmap = loaded;
        m_loadedSource = m_source;
    }
    QPixmap pixmap = m_pixmap;
    const int w = qMax(1, m_width), h = qMax(1, m_height);
    if (pixmap.width() != w || pixmap.height() != h) {
        pixmap = pixmap.scaled(w, h, Qt::KeepAspectRatio, Qt::FastTransformation);
    }
    m_target->setCursor(QCursor(pixmap, qBound(0, m_hotX, pixmap.width() - 1),
                                qBound(0, m_hotY, pixmap.height() - 1)));
}
