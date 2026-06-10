#include "video_helper.h"
#include <QtMultimedia/QVideoSink>
#include <QtMultimedia/QVideoFrame>
#include <QtMultimedia/QVideoFrameFormat>
#include <QCursor>
#include <QGuiApplication>
#include <QWindow>
#include <QEvent>
#include <QMouseEvent>
#include <QWheelEvent>
#include <QMetaObject>
#include <QPointer>
#include <QThread>
#include <cstring>
#include <iostream>
#include <cmath>
#include <vector>

void deliver_yuv_frame(QVideoSink* sink,
                       const uint8_t* y_data, int y_stride,
                       const uint8_t* u_data, int u_stride,
                       const uint8_t* v_data, int v_stride,
                       int width, int height) {
    if (!sink) return;

    QVideoFrameFormat format(QSize(width, height), QVideoFrameFormat::Format_YUV420P);
    QVideoFrame frame(format);

    if (frame.map(QVideoFrame::WriteOnly)) {
        // Copy Y plane
        uint8_t* dst_y = frame.bits(0);
        int dst_y_stride = frame.bytesPerLine(0);
        if (dst_y_stride == width && y_stride == width) {
            std::memcpy(dst_y, y_data, width * height);
        } else {
            for (int i = 0; i < height; ++i) {
                std::memcpy(dst_y + i * dst_y_stride, y_data + i * y_stride, width);
            }
        }

        // Copy U plane
        uint8_t* dst_u = frame.bits(1);
        int dst_u_stride = frame.bytesPerLine(1);
        int uv_height = height / 2;
        int uv_width = width / 2;
        if (dst_u_stride == uv_width && u_stride == uv_width) {
            std::memcpy(dst_u, u_data, uv_width * uv_height);
        } else {
            for (int i = 0; i < uv_height; ++i) {
                std::memcpy(dst_u + i * dst_u_stride, u_data + i * u_stride, uv_width);
            }
        }

        // Copy V plane
        uint8_t* dst_v = frame.bits(2);
        int dst_v_stride = frame.bytesPerLine(2);
        if (dst_v_stride == uv_width && v_stride == uv_width) {
            std::memcpy(dst_v, v_data, uv_width * uv_height);
        } else {
            for (int i = 0; i < uv_height; ++i) {
                std::memcpy(dst_v + i * dst_v_stride, v_data + i * v_stride, uv_width);
            }
        }

        frame.unmap();

        if (sink->thread() == QThread::currentThread()) {
            sink->setVideoFrame(frame);
        } else {
            QPointer<QVideoSink> sink_guard(sink);
            QMetaObject::invokeMethod(
                sink,
                [sink_guard, frame]() mutable {
                    if (sink_guard) {
                        sink_guard->setVideoFrame(frame);
                    }
                },
                Qt::DirectConnection);
        }
    }
}

void warp_cursor_helper(int x, int y) {
    QCursor::setPos(x, y);
}

#if defined(Q_OS_LINUX)
#include <dlfcn.h>

typedef int (*XGrabKeyboardFn)(void*, unsigned long, int, int, int, unsigned long);
typedef int (*XUngrabKeyboardFn)(void*, unsigned long);
#endif

void set_keyboard_grab_helper(bool grab) {
    QWindow* window = nullptr;
    auto windows = QGuiApplication::allWindows();
    if (!windows.isEmpty()) {
        window = windows.first();
    }
    std::cerr << "Lunaris Client: set_keyboard_grab_helper(" << (grab ? "true" : "false") << ") - Target Window: " << window << std::endl;
    if (!window) return;

    if (grab) {
        window->requestActivate();
    }

#if defined(Q_OS_LINUX)
    auto* x11App = qApp->nativeInterface<QNativeInterface::QX11Application>();
    if (x11App) {
        void* display = x11App->display();
        unsigned long x_window = (unsigned long)window->winId();
        if (display && x_window) {
            void* libX11 = dlopen("libX11.so.6", RTLD_LAZY | RTLD_NOLOAD);
            if (!libX11) libX11 = dlopen("libX11.so", RTLD_LAZY | RTLD_NOLOAD);
            if (!libX11) libX11 = dlopen("libX11.so.6", RTLD_LAZY);
            if (!libX11) libX11 = dlopen("libX11.so", RTLD_LAZY);

            if (libX11) {
                auto grab_fn = (XGrabKeyboardFn)dlsym(libX11, "XGrabKeyboard");
                auto ungrab_fn = (XUngrabKeyboardFn)dlsym(libX11, "XUngrabKeyboard");
                if (grab_fn && ungrab_fn) {
                    if (grab) {
                        int result = grab_fn(display, x_window, 1, 1, 1, 0); // GrabModeAsync=1, CurrentTime=0
                        std::cerr << "Lunaris Client: Native XGrabKeyboard returned: " << result << std::endl;
                    } else {
                        ungrab_fn(display, 0); // CurrentTime=0
                        std::cerr << "Lunaris Client: Native XUngrabKeyboard called" << std::endl;
                    }
                    return;
                }
            }
        }
    }
#endif

    // Fallback if not Linux/X11 or if native loading failed
    bool success = window->setKeyboardGrabEnabled(grab);
    std::cerr << "Lunaris Client: Fallback setKeyboardGrabEnabled returned: " << (success ? "true" : "false") << std::endl;
}

static bool g_pointerLocked = false;
static QObject* g_streamBridge = nullptr;
static int g_windowWidth = 1280;
static int g_windowHeight = 720;
static int g_centerX = 640;
static int g_centerY = 360;
static int g_globalCenterX = 640;
static int g_globalCenterY = 360;
static int g_lastGlobalX = 640;
static int g_lastGlobalY = 360;
static int g_pendingWarps = 0;

static int getButtonCode(Qt::MouseButton btn) {
    if (btn == Qt::LeftButton) return 1;
    if (btn == Qt::MiddleButton) return 2;
    if (btn == Qt::RightButton) return 3;
    return 0;
}

#if defined(Q_OS_WIN)
#include <QAbstractNativeEventFilter>
#include <windows.h>

void register_raw_input(HWND hwnd) {
    RAWINPUTDEVICE rid;
    rid.usUsagePage = 0x01; // Generic Desktop Page
    rid.usUsage = 0x02;     // Mouse
    rid.dwFlags = RIDEV_INPUTSINK;
    rid.hwndTarget = hwnd;
    if (!RegisterRawInputDevices(&rid, 1, sizeof(rid))) {
        std::cerr << "Lunaris: Failed to register Windows raw input devices." << std::endl;
    } else {
        std::cerr << "Lunaris: Registered Windows Raw Input." << std::endl;
    }
}

class WindowsRawInputFilter : public QAbstractNativeEventFilter {
public:
    bool nativeEventFilter(const QByteArray &eventType, void *message, qintptr *result) override {
        if (g_pointerLocked && g_streamBridge && eventType == "windows_generic_MSG") {
            MSG* msg = static_cast<MSG*>(message);
            if (msg->message == WM_INPUT) {
                UINT dwSize = 0;
                GetRawInputData((HRAWINPUT)msg->lParam, RID_INPUT, NULL, &dwSize, sizeof(RAWINPUTHEADER));
                if (dwSize > 0) {
                    std::vector<BYTE> lparam_buf(dwSize);
                    if (GetRawInputData((HRAWINPUT)msg->lParam, RID_INPUT, lparam_buf.data(), &dwSize, sizeof(RAWINPUTHEADER)) == dwSize) {
                        RAWINPUT* raw = (RAWINPUT*)lparam_buf.data();
                        if (raw->header.dwType == RIM_TYPEMOUSE) {
                            int rx = raw->data.mouse.lLastX;
                            int ry = raw->data.mouse.lLastY;
                            USHORT flags = raw->data.mouse.usFlags;

                            // Only process relative movement
                            if ((flags & MOUSE_MOVE_ABSOLUTE) == 0) {
                                if (rx != 0 || ry != 0) {
                                    QMetaObject::invokeMethod(g_streamBridge, "sendMouseMove",
                                                              Qt::DirectConnection,
                                                              Q_ARG(::std::int32_t, 0),
                                                              Q_ARG(::std::int32_t, 0),
                                                              Q_ARG(::std::int32_t, g_windowWidth),
                                                              Q_ARG(::std::int32_t, g_windowHeight),
                                                              Q_ARG(::std::int32_t, rx),
                                                              Q_ARG(::std::int32_t, ry),
                                                              Q_ARG(bool, true));
                                }
                            }
                        }
                    }
                }
                return true; // Consume raw mouse input while pointer lock owns relative motion.
            }
        }
        return false; // Let Qt process standard messages
    }
};

static WindowsRawInputFilter* g_nativeEventFilter = nullptr;
#endif

class InputEventFilter : public QObject {
protected:
    bool eventFilter(QObject* watched, QEvent* event) override {
        if (!g_pointerLocked || !g_streamBridge) {
            return QObject::eventFilter(watched, event);
        }

        if (event->type() == QEvent::MouseMove) {
            QMouseEvent* me = static_cast<QMouseEvent*>(event);
            
            QWindow* window = QGuiApplication::focusWindow();
            if (!window) {
                auto windows = QGuiApplication::allWindows();
                if (!windows.isEmpty()) window = windows.first();
            }
            if (window) {
                int oldGlobalX = g_globalCenterX;
                int oldGlobalY = g_globalCenterY;
                g_windowWidth = window->width();
                g_windowHeight = window->height();
                g_centerX = g_windowWidth / 2;
                g_centerY = g_windowHeight / 2;
                QPoint gCenter = window->mapToGlobal(QPoint(g_centerX, g_centerY));
                g_globalCenterX = gCenter.x();
                g_globalCenterY = gCenter.y();

                if (g_globalCenterX != oldGlobalX || g_globalCenterY != oldGlobalY) {
                    g_lastGlobalX = g_globalCenterX;
                    g_lastGlobalY = g_globalCenterY;
                    g_pendingWarps = 0;
                }
            }

#if defined(Q_OS_WIN)
            // On Windows, WindowsRawInputFilter handles relative mouse moves.
            // We only keep the cursor confined to the center of the window to avoid escaping.
            int dx = me->position().x() - g_centerX;
            int dy = me->position().y() - g_centerY;
            if (std::abs(dx) > g_windowWidth / 4 || std::abs(dy) > g_windowHeight / 4) {
                QCursor::setPos(g_globalCenterX, g_globalCenterY);
            }
            return true; // Consume the event so QML doesn't process it
#else
            // On Linux/macOS, measure mouse delta using global coordinates
            // to avoid warp-loop feedback and double-counting during fast movements.
            int gx = std::round(me->globalPosition().x());
            int gy = std::round(me->globalPosition().y());

            // Check if this matches a warp event. Some compositors report the
            // synthetic move a pixel or two away from the requested center.
            if (g_pendingWarps > 0 && std::abs(gx - g_globalCenterX) <= 2 && std::abs(gy - g_globalCenterY) <= 2) {
                g_pendingWarps--;
                g_lastGlobalX = g_globalCenterX;
                g_lastGlobalY = g_globalCenterY;
                return true;
            }

            int rx = gx - g_lastGlobalX;
            int ry = gy - g_lastGlobalY;

            g_lastGlobalX = gx;
            g_lastGlobalY = gy;

            if (rx != 0 || ry != 0) {
                QMetaObject::invokeMethod(g_streamBridge, "sendMouseMove",
                                          Qt::DirectConnection,
                                          Q_ARG(::std::int32_t, (::std::int32_t)me->position().x()),
                                          Q_ARG(::std::int32_t, (::std::int32_t)me->position().y()),
                                          Q_ARG(::std::int32_t, g_windowWidth),
                                          Q_ARG(::std::int32_t, g_windowHeight),
                                          Q_ARG(::std::int32_t, rx),
                                          Q_ARG(::std::int32_t, ry),
                                          Q_ARG(bool, true));
            }

            int dx_from_center = me->position().x() - g_centerX;
            int dy_from_center = me->position().y() - g_centerY;
            if (std::abs(dx_from_center) > g_windowWidth / 4 || std::abs(dy_from_center) > g_windowHeight / 4) {
                g_lastGlobalX = g_globalCenterX;
                g_lastGlobalY = g_globalCenterY;
                g_pendingWarps++;
                QCursor::setPos(g_globalCenterX, g_globalCenterY);
            }
            return true;
#endif
        }

        if (event->type() == QEvent::MouseButtonPress) {
            QMouseEvent* me = static_cast<QMouseEvent*>(event);
            QMetaObject::invokeMethod(g_streamBridge, "sendMouseClick",
                                      Qt::DirectConnection,
                                      Q_ARG(::std::int32_t, getButtonCode(me->button())),
                                      Q_ARG(bool, true));
            return true;
        }

        if (event->type() == QEvent::MouseButtonRelease) {
            QMouseEvent* me = static_cast<QMouseEvent*>(event);
            QMetaObject::invokeMethod(g_streamBridge, "sendMouseClick",
                                      Qt::DirectConnection,
                                      Q_ARG(::std::int32_t, getButtonCode(me->button())),
                                      Q_ARG(bool, false));
            return true;
        }

        if (event->type() == QEvent::Wheel) {
            QWheelEvent* we = static_cast<QWheelEvent*>(event);
            QMetaObject::invokeMethod(g_streamBridge, "sendMouseWheel",
                                      Qt::DirectConnection,
                                      Q_ARG(::std::int32_t, we->angleDelta().y()));
            return true;
        }

        return QObject::eventFilter(watched, event);
    }
};

static InputEventFilter* g_eventFilter = nullptr;

void register_bridge_instance(StreamBridge* bridge) {
    g_streamBridge = (QObject*)bridge;
    std::cerr << "Lunaris Client: register_bridge_instance - Registered active bridge pointer." << std::endl;
}

static bool g_cursorOverrideActive = false;

void set_pointer_locked_helper(bool locked) {
    g_pointerLocked = locked;
    std::cerr << "Lunaris Client: set_pointer_locked_helper(" << (locked ? "true" : "false") << ")" << std::endl;
    
    if (locked) {
        if (!g_cursorOverrideActive) {
            QGuiApplication::setOverrideCursor(QCursor(Qt::BlankCursor));
            g_cursorOverrideActive = true;
        }

        if (!g_eventFilter) {
            g_eventFilter = new InputEventFilter();
        }
        QGuiApplication::instance()->installEventFilter(g_eventFilter);

        QWindow* window = QGuiApplication::focusWindow();
        if (!window) {
            auto windows = QGuiApplication::allWindows();
            if (!windows.isEmpty()) window = windows.first();
        }
        if (window) {
            g_windowWidth = window->width();
            g_windowHeight = window->height();
            g_centerX = g_windowWidth / 2;
            g_centerY = g_windowHeight / 2;
            QPoint gCenter = window->mapToGlobal(QPoint(g_centerX, g_centerY));
            g_globalCenterX = gCenter.x();
            g_globalCenterY = gCenter.y();

            // Initial warp
            g_lastGlobalX = g_globalCenterX;
            g_lastGlobalY = g_globalCenterY;
            g_pendingWarps = 1;
            QCursor::setPos(g_globalCenterX, g_globalCenterY);

#if defined(Q_OS_WIN)
            if (!g_nativeEventFilter) {
                g_nativeEventFilter = new WindowsRawInputFilter();
            }
            QGuiApplication::instance()->installNativeEventFilter(g_nativeEventFilter);
            register_raw_input((HWND)window->winId());
#endif
        }
    } else {
        if (g_cursorOverrideActive) {
            QGuiApplication::restoreOverrideCursor();
            g_cursorOverrideActive = false;
        }

        if (g_eventFilter) {
            QGuiApplication::instance()->removeEventFilter(g_eventFilter);
        }
#if defined(Q_OS_WIN)
        if (g_nativeEventFilter) {
            QGuiApplication::instance()->removeNativeEventFilter(g_nativeEventFilter);
        }
#endif
        g_pendingWarps = 0;
    }
}
