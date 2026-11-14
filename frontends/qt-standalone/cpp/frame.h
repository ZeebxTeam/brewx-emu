// The item that puts the game frame in the Qt Quick scene graph. It is the base of the Rust
// `GameScreen`: cxx-qt has no `QSGNode` binding, and `updatePaintNode` needs one.
#pragma once

#include <QtGui/QImage>
#include <QtQuick/QQuickItem>

class FrameItem : public QQuickItem
{
    Q_OBJECT

public:
    explicit FrameItem(QQuickItem *parent = nullptr);

    // A new frame; it is drawn as large as fits, keeping the aspect ratio, without smoothing.
    void showImage(const QImage &image);
    void clear();

protected:
    QSGNode *updatePaintNode(QSGNode *old, UpdatePaintNodeData *) override;
    void geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) override;

private:
    QImage m_image;
    bool m_newImage = false;
};
