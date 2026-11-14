#include "frame.h"

#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGSimpleTextureNode>

FrameItem::FrameItem(QQuickItem *parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
}

void FrameItem::showImage(const QImage &image)
{
    m_image = image;
    m_newImage = true;
    update();
}

void FrameItem::clear()
{
    m_image = QImage();
    update();
}

void FrameItem::geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry)
{
    QQuickItem::geometryChange(newGeometry, oldGeometry);
    update();
}

QSGNode *FrameItem::updatePaintNode(QSGNode *old, UpdatePaintNodeData *)
{
    auto *node = static_cast<QSGSimpleTextureNode *>(old);
    if (m_image.isNull() || window() == nullptr) {
        delete node;
        return nullptr;
    }
    if (node == nullptr) {
        node = new QSGSimpleTextureNode;
        node->setOwnsTexture(true);
    }
    // Uploaded only when the game pushed a new frame; a resize just moves the quad.
    if (m_newImage || node->texture() == nullptr) {
        node->setTexture(window()->createTextureFromImage(m_image));
        m_newImage = false;
    }
    const qreal scale = qMin(width() / m_image.width(), height() / m_image.height());
    const QSizeF size(m_image.width() * scale, m_image.height() * scale);
    node->setRect(QRectF(QPointF((width() - size.width()) / 2, (height() - size.height()) / 2), size));
    node->setFiltering(QSGTexture::Nearest);
    return node;
}
