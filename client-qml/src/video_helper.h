#pragma once
#include <QtMultimedia/QVideoSink>
#include <cstdint>
#include <QObject>

// Forward declare StreamBridge
class StreamBridge;

void deliver_yuv_frame(QVideoSink* sink,
                       const uint8_t* y_data, int y_stride,
                       const uint8_t* u_data, int u_stride,
                       const uint8_t* v_data, int v_stride,
                       int width, int height);

void deliver_bgrx_frame(QVideoSink* sink, const uint8_t* data, int stride,
                        int width, int height);

// Colours of the decoded frames (from the stream, set per frame by the
// decoder): BT.709 or BT.601 matrix, full (0-255) or studio (16-235) range.
// Every renderer converts YCbCr to RGB with these.
void set_video_color(bool bt709, bool full_range);

struct YuvCoefficients {
    float y_offset; // subtracted from Y (0..1)
    float y_scale;  // then Y is scaled by this
    float c_scale;  // Cb/Cr (centred) are scaled by this
    float rv, gu, gv, bu; // R = Y + rv*Cr, G = Y - gu*Cb - gv*Cr, B = Y + bu*Cb
};
YuvCoefficients video_yuv_coefficients();
bool video_color_bt709();

// The encoderCscMode asked of the host for this session (bit 0 = full range,
// bits 1.. = colourspace 0 Rec. 601, 1 Rec. 709, 2 Rec. 2020), set by the
// Rust side when the session is requested. Untagged frames are assumed to have
// it (moonlight-qt renderer.h:245-275).
void set_requested_csc_mode(int mode);
int requested_csc_mode();
bool video_color_full_range();

void warp_cursor_helper(int x, int y);
// Refresh rate in Hz of the screen the app's window is on (the primary screen
// before a window exists), 0 when unknown.
double window_screen_refresh_rate();
void set_keyboard_grab_helper(bool grab);

void register_bridge_instance(StreamBridge* bridge);
void set_pointer_locked_helper(bool locked);

// Creates the process-wide QApplication. A QtWidgets application (not just
// QGuiApplication) is required because Qt.labs.platform's SystemTrayIcon/Menu
// fall back to QtWidgets implementations on desktops like KDE, and creating
// those without a QApplication aborts the process.
void create_widgets_application();
int exec_widgets_application();
