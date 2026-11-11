#ifndef _AEROS_LINUX_PCI_P2PDMA_H
#define _AEROS_LINUX_PCI_P2PDMA_H

#include <linux/pci.h>
static inline int pci_p2pdma_distance(struct pci_dev *provider, struct device *client, bool verbose)
{
	(void)provider; (void)client; (void)verbose;
	return -1;
}
static inline int pci_p2pdma_distance_many(struct pci_dev *provider, struct device **clients, int num_clients,
					   bool verbose)
{
	(void)provider; (void)clients; (void)num_clients; (void)verbose;
	return -1;
}
#endif
