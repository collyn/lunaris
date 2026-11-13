// Start-up check for the client AppImage: a QML VideoOutput must get a working
// QVideoSink. On Linux Qt Multimedia only provides one when its FFmpeg media plugin
// (plugins/multimedia/libffmpegmediaplugin.so and the libav* it links) is deployed
// and loads; without it QVideoSink drops every frame ("Failed to create QVideoSink",
// "No QtMultimedia backends found") and the client's CPU video path shows nothing.
// Built and run by videosink-check.sh. Exit code 0 = works.
#include <QtGui>
#include <QtQuick>
#include <QtMultimedia>
#include <cstdio>

int main(int argc, char **argv) {
    QGuiApplication app(argc, argv);
    QQuickView view;
    view.setSource(QUrl::fromLocalFile(QCoreApplication::applicationDirPath() + "/videosink-check.qml"));
    if (view.status() != QQuickView::Ready) { printf("FAIL: QML did not load\n"); return 2; }
    view.resize(200, 200);
    view.show();
    for (int i = 0; i < 40 && !view.isExposed(); ++i) { app.processEvents(); QThread::msleep(50); }
    QObject *output = view.rootObject()->findChild<QObject *>("output");
    auto *sink = output ? output->property("videoSink").value<QVideoSink *>() : nullptr;
    if (!sink) { printf("FAIL: VideoOutput has no videoSink\n"); return 3; }

    // The same kind of frame the client delivers on its CPU path: planar YUV in memory.
    QVideoFrame frame{QVideoFrameFormat(QSize(64, 64), QVideoFrameFormat::Format_YUV420P)};
    frame.map(QVideoFrame::WriteOnly);
    for (int plane = 0; plane < frame.planeCount(); ++plane)
        memset(frame.bits(plane), 128, size_t(frame.bytesPerLine(plane)) * (plane ? 32 : 64));
    frame.unmap();
    sink->setVideoFrame(frame);
    for (int i = 0; i < 20; ++i) { app.processEvents(); QThread::msleep(50); }
    if (!sink->videoFrame().isValid()) { printf("FAIL: the sink dropped the frame (no media backend plugin)\n"); return 4; }
    printf("OK: VideoOutput took a %dx%d frame\n", sink->videoFrame().width(), sink->videoFrame().height());
    return 0;
}
