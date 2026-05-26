#pragma once
#include <QtMultimedia/QVideoSink>
#include <cstdint>

void deliver_yuv_frame(QVideoSink* sink,
                       const uint8_t* y_data, int y_stride,
                       const uint8_t* u_data, int u_stride,
                       const uint8_t* v_data, int v_stride,
                       int width, int height);

void warp_cursor_helper(int x, int y);
void set_keyboard_grab_helper(bool grab);

