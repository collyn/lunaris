#pragma once

#include <QObject>
#include <QPixmap>
#include <QPointer>
#include <QQuickItem>
#include <QString>

// Shows the remote host's cursor as the real OS cursor over `target`.
//
// Drawing the cursor as a QML Image follows the mouse only at the scene
// graph's frame rate (plus the compositor's frame), which makes it trail the
// hand by a couple of frames. A hardware cursor is moved by the compositor
// at input rate, with no lag. While `active`, the target item's cursor is a
// pixmap of `source` (a "qrc:/..." asset or a "data:image/png;base64,..." URL)
// scaled to cursorWidth x cursorHeight with the given hotspot, or a blank
// cursor when `hidden`.
class HostCursor : public QObject {
    Q_OBJECT
    Q_PROPERTY(QQuickItem *target READ target WRITE setTarget NOTIFY changed)
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY changed)
    Q_PROPERTY(bool hidden READ hidden WRITE setHidden NOTIFY changed)
    Q_PROPERTY(QString source READ source WRITE setSource NOTIFY changed)
    Q_PROPERTY(int cursorWidth READ cursorWidth WRITE setCursorWidth NOTIFY changed)
    Q_PROPERTY(int cursorHeight READ cursorHeight WRITE setCursorHeight NOTIFY changed)
    Q_PROPERTY(int hotspotX READ hotspotX WRITE setHotspotX NOTIFY changed)
    Q_PROPERTY(int hotspotY READ hotspotY WRITE setHotspotY NOTIFY changed)

public:
    explicit HostCursor(QObject *parent = nullptr);

    QQuickItem *target() const { return m_target; }
    bool active() const { return m_active; }
    bool hidden() const { return m_hidden; }
    QString source() const { return m_source; }
    int cursorWidth() const { return m_width; }
    int cursorHeight() const { return m_height; }
    int hotspotX() const { return m_hotX; }
    int hotspotY() const { return m_hotY; }

    void setTarget(QQuickItem *target);
    void setActive(bool active);
    void setHidden(bool hidden);
    void setSource(const QString &source);
    void setCursorWidth(int width);
    void setCursorHeight(int height);
    void setHotspotX(int x);
    void setHotspotY(int y);

    static void registerTypes();

signals:
    void changed();

private slots:
    void apply();

private:
    void scheduleApply();

    QPointer<QQuickItem> m_target;
    bool m_active = false;
    bool m_hidden = false;
    QString m_source;
    int m_width = 32;
    int m_height = 32;
    int m_hotX = 0;
    int m_hotY = 0;
    bool m_applyQueued = false;
    QString m_loadedSource;
    QPixmap m_pixmap;
};
