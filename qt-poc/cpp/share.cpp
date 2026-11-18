#include "share.h"

#include <QtCore/QCoreApplication>

void shareGlContexts()
{
    QCoreApplication::setAttribute(Qt::AA_ShareOpenGLContexts);
}
