#ifndef AGENT_GUI_H
#define AGENT_GUI_H

#include <QString>

// Opens a URL in the platform's default handler (browser / protocol handler).
void open_url_helper(const QString &url);

// Opens a native file dialog for importing a JSON agent config.
// Returns the absolute file path, or an empty string if cancelled.
QString pick_import_file();

#endif
