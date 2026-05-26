#include <QtMultimedia/QVideoSink>
#include <QtMultimedia/QVideoFrame>
#include <QtMultimedia/QVideoFrameFormat>
#include <QCursor>
#include <QGuiApplication>
#include <QWindow>
#include <cstring>
#include <iostream>

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
        for (int i = 0; i < height; ++i) {
            std::memcpy(dst_y + i * dst_y_stride, y_data + i * y_stride, width);
        }

        // Copy U plane
        uint8_t* dst_u = frame.bits(1);
        int dst_u_stride = frame.bytesPerLine(1);
        int uv_height = height / 2;
        int uv_width = width / 2;
        for (int i = 0; i < uv_height; ++i) {
            std::memcpy(dst_u + i * dst_u_stride, u_data + i * u_stride, uv_width);
        }

        // Copy V plane
        uint8_t* dst_v = frame.bits(2);
        int dst_v_stride = frame.bytesPerLine(2);
        for (int i = 0; i < uv_height; ++i) {
            std::memcpy(dst_v + i * dst_v_stride, v_data + i * v_stride, uv_width);
        }

        frame.unmap();
        sink->setVideoFrame(frame);
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
